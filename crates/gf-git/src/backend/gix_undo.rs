//! Undo captures through gix: the app captures before and after every
//! operation it records, and the CLI needs a git process for each part
//! (HEAD, index, files, refs, stash, every config section). Must equal the
//! CLI's [`UndoOps::capture`] (see `tests/gix_contract.rs`); restoring
//! stays on the CLI.

use std::path::{Component, Path, PathBuf};

use gf_core::Result;
use gix::bstr::{BStr, ByteSlice};
use gix::refs::TargetRef;

use super::gix_backend::{err, head_id, reflog_lines, text, unsupported};
use super::{GixBackend, UndoOps};
use crate::cli::parse::bytes_to_path;
use crate::ops::undo::{
    Capture, FileContent, Head, HeadMode, IndexEntry, Scope, StashItem, executable,
};

/// `symbolic-ref --short HEAD` and `rev-parse --verify HEAD`.
fn head(repo: &gix::Repository) -> Result<Option<Head>> {
    let branch = repo
        .head()
        .map_err(err)?
        .referent_name()
        .map(|n| text(n.shorten()));
    let oid = head_id(repo)?.map(|id| id.to_string());
    Ok((branch.is_some() || oid.is_some()).then_some(Head { branch, oid }))
}

/// A scope path as `ls-files` takes it literally: `(path, folder only)`.
/// `None` for anything that is not plainly inside the work tree.
fn literal(p: &Path) -> Option<(Vec<u8>, bool)> {
    let s = p.to_str()?;
    let dir_only = s.ends_with('/');
    let s = s.trim_end_matches('/');
    let plain = !s.is_empty()
        && Path::new(s)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    plain.then(|| (s.replace('\\', "/").into_bytes(), dir_only))
}

/// `ls-files -s` of `paths` (literal pathspecs: a path, or a folder and
/// everything under it), sorted.
fn index_entries(repo: &gix::Repository, paths: &[PathBuf]) -> Result<Vec<IndexEntry>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut specs = Vec::new();
    for p in paths {
        match literal(p) {
            Some(spec) => specs.push(spec),
            None => return unsupported("index path that is not plain"),
        }
    }
    let index = repo.index_or_empty().map_err(err)?;
    if index.is_sparse() {
        return unsupported("sparse index");
    }
    let matches = |path: &BStr| {
        specs.iter().any(|(spec, dir_only)| {
            let under =
                path.len() > spec.len() && path.starts_with(spec) && path[spec.len()] == b'/';
            under || (!dir_only && path == spec.as_slice())
        })
    };
    let mut entries: Vec<IndexEntry> = index
        .entries()
        .iter()
        .filter(|e| matches(e.path(&index)))
        .map(|e| IndexEntry {
            path: bytes_to_path(e.path(&index).as_bytes()),
            stage: u8::try_from(e.stage_raw()).unwrap_or(u8::MAX),
            mode: format!("{:06o}", e.mode.bits()),
            oid: e.id.to_string(),
        })
        .collect();
    entries.sort();
    Ok(entries)
}

/// Work tree files saved as blobs, exactly as `hash-object -w
/// --no-filters` stores them.
fn files(repo: &gix::Repository, paths: &[PathBuf]) -> Result<Vec<(PathBuf, Option<FileContent>)>> {
    let Some(workdir) = repo.workdir() else {
        return unsupported("files of a bare repository");
    };
    let mut out = Vec::new();
    for rel in paths {
        let abs = workdir.join(rel);
        let content = match std::fs::symlink_metadata(&abs) {
            Ok(m) if m.file_type().is_symlink() => {
                Some(FileContent::Symlink(std::fs::read_link(&abs)?))
            }
            Ok(m) if m.is_file() => {
                let bytes = std::fs::read(&abs)?;
                Some(FileContent::Blob {
                    oid: repo.write_blob(bytes).map_err(err)?.to_string(),
                    executable: executable(&m),
                })
            }
            _ => None,
        };
        out.push((rel.clone(), content));
    }
    Ok(out)
}

/// `for-each-ref --format=%(objectname)` of each full name (the ref's own
/// target: an annotated tag stays the tag object).
fn refs(repo: &gix::Repository, names: &[String]) -> Result<Vec<(String, Option<String>)>> {
    let mut out = Vec::new();
    for name in names {
        let id = if name.starts_with("refs/") {
            match repo.try_find_reference(name.as_str()).map_err(err)? {
                // Lookup may fall back to other prefixes: only this name.
                Some(r) if r.name().as_bstr() == name.as_bytes() => match r.target() {
                    TargetRef::Object(id) => Some(id.to_string()),
                    TargetRef::Symbolic(_) => return unsupported("symbolic ref in scope"),
                },
                _ => None,
            }
        } else {
            None
        };
        out.push((name.clone(), id));
    }
    Ok(out)
}

/// `git config --local --get-regexp '^<section>\.'`, this section's own
/// keys only, in file order (no includes, as with `--local`).
fn config(repo: &gix::Repository, sections: &[String]) -> Result<Vec<(String, String)>> {
    if sections.is_empty() {
        return Ok(Vec::new());
    }
    let path = repo.common_dir().join("config");
    let file = match gix::config::File::from_path_no_includes(path, gix::config::Source::Local) {
        Ok(file) => file,
        Err(_) if !repo.common_dir().join("config").exists() => return Ok(Vec::new()),
        Err(e) => return Err(err(e)),
    };
    let mut out = Vec::new();
    for wanted in sections {
        let (name, sub) = match wanted.split_once('.') {
            Some((name, sub)) => (name, Some(sub)),
            None => (wanted.as_str(), None),
        };
        for section in file.sections() {
            let header = section.header();
            if !header.name().eq_ignore_ascii_case(name.as_bytes())
                || header.subsection_name().map(|s| s.as_bytes()) != sub.map(str::as_bytes)
            {
                continue;
            }
            if header.is_legacy() {
                return unsupported("legacy config section header");
            }
            let values: Vec<(String, gix::bstr::BString)> = section.body().into_iter().collect();
            // A key without `=` means true to git; gix reads it as missing
            // or empty, and cannot tell it from `key =`. Leave both to git.
            if values.len() != section.value_names().count()
                || values.iter().any(|(_, v)| v.is_empty())
            {
                return unsupported("config key without a value");
            }
            let prefix = match sub {
                Some(sub) => format!("{}.{sub}", name.to_ascii_lowercase()),
                None => name.to_ascii_lowercase(),
            };
            for (key, value) in values {
                out.push((
                    format!("{prefix}.{}", key.to_ascii_lowercase()),
                    text(value.as_ref()),
                ));
            }
        }
    }
    Ok(out)
}

fn capture(repo: &gix::Repository, scope: &Scope) -> Result<Capture> {
    if repo.object_hash() != gix::hash::Kind::Sha1 {
        return unsupported("capture in a non-SHA-1 repository");
    }
    let head_now = if scope.head { head(repo)? } else { None };
    let mut submodules = Vec::new();
    if !scope.submodules.is_empty() {
        let Some(workdir) = repo.workdir() else {
            return unsupported("submodules of a bare repository");
        };
        for p in &scope.submodules {
            let dir = workdir.join(p);
            let head = if dir.join(".git").exists() {
                match gix::open(&dir) {
                    Ok(sub) => head(&sub)?,
                    Err(_) => return unsupported("submodule gix cannot open"),
                }
            } else {
                None
            };
            submodules.push((p.clone(), head));
        }
    }
    let stash = if scope.stash {
        reflog_lines(repo, "refs/stash", usize::MAX)?
            .into_iter()
            .map(|l| StashItem {
                oid: l.new_oid.to_string(),
                message: text(l.message.as_ref()),
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(Capture {
        head: head_now,
        index: index_entries(repo, &scope.index)?,
        files: files(repo, &scope.files)?,
        refs: refs(repo, &scope.refs)?,
        stash,
        config: config(repo, &scope.config)?,
        submodules,
    })
}

impl UndoOps for GixBackend {
    async fn stash_paths(&self, _: usize) -> Result<Vec<PathBuf>> {
        unsupported("stash paths")
    }

    async fn capture(&self, scope: &Scope) -> Result<Capture> {
        let scope = scope.clone();
        self.blocking(move |repo| capture(repo, &scope)).await
    }

    async fn restore(
        &self,
        _: &Scope,
        _: Option<&Capture>,
        _: &Capture,
        _: HeadMode,
    ) -> Result<()> {
        unsupported("undo restore")
    }

    async fn pin_saved_files(&self, _: &[String]) -> Result<()> {
        unsupported("pin saved files")
    }

    async fn published_leaving(&self, _: &str, _: &str, _: Option<&str>) -> Result<usize> {
        unsupported("published commits")
    }
}
