//! Undo and redo for gf's own operations.
//!
//! Before an operation runs, the app captures the state of what it is about
//! to touch (its [`Scope`]); after it ran, it captures the same scope again.
//! Undo restores the first [`Capture`], redo the second — each only after
//! checking that the repository still looks like the other one (the
//! *guard*), so nothing done since (in gf, an editor or the terminal) is
//! overwritten silently.
//!
//! What a scope can cover, and how it is put back:
//!
//! | aspect      | captured as                              | restored with                               |
//! |-------------|------------------------------------------|---------------------------------------------|
//! | HEAD        | branch + commit                          | `reset --soft/--keep`, `switch`, `symbolic-ref` |
//! | index       | `ls-files -s` entries of some paths      | `update-index --index-info` (conflict stages too) |
//! | work tree   | file bytes stored as blobs (`hash-object -w --no-filters`) | the blob bytes written back      |
//! | refs        | branch / tag ids (annotated tags kept)   | `update-ref` / `update-ref -d`              |
//! | stash       | stash commits + messages                 | `stash store` / `stash drop`                |
//! | config      | whole sections (e.g. `branch.main`)      | `config --remove-section` + `--add`         |
//! | submodules  | checked-out branch + commit              | `checkout` in the submodule                 |
//!
//! Blobs written for the work tree are not referenced by any ref; `git gc`
//! keeps such objects for two weeks (`gc.pruneExpire`), which bounds how
//! long a saved discard can be brought back.

use std::path::{Path, PathBuf};

use gf_core::{GfError, Result};
use serde::{Deserialize, Serialize};

use crate::backend::{CliBackend, UndoOps};
use crate::cli::GitCommand;

/// Work tree files larger than this in total are not saved: the operation
/// runs without an undo step (the app says so before it runs).
pub const MAX_SAVED_BYTES: u64 = 64 * 1024 * 1024;
/// Same for the number of files.
pub const MAX_SAVED_FILES: usize = 5000;

/// What an operation touches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    /// HEAD: the checked-out branch and commit.
    pub head: bool,
    /// Index entries of these paths (all stages).
    pub index: Vec<PathBuf>,
    /// Work tree content of these files (see [`Scope::expand_files`]).
    pub files: Vec<PathBuf>,
    /// Full ref names, e.g. `refs/heads/main`, `refs/tags/v1`.
    pub refs: Vec<String>,
    /// The stash list.
    pub stash: bool,
    /// Whole config sections, e.g. `branch.main`.
    pub config: Vec<String>,
    /// Submodules (paths): which commit/branch is checked out in them.
    pub submodules: Vec<PathBuf>,
}

impl Scope {
    pub fn head() -> Self {
        Self {
            head: true,
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Replaces directories in `files` by the files under them, so a later
    /// capture (after the directory is gone) covers the same files.
    /// Fails with [`GfError::Unsupported`] when there is too much to save.
    pub fn expand_files(&mut self, workdir: &Path) -> Result<()> {
        let mut out = Vec::new();
        let mut bytes = 0u64;
        let mut stack: Vec<PathBuf> = self.files.drain(..).rev().collect();
        while let Some(rel) = stack.pop() {
            let abs = workdir.join(&rel);
            match std::fs::symlink_metadata(&abs) {
                Ok(m) if m.is_dir() => {
                    // A nested repository (submodule or not) is not ours.
                    if abs.join(".git").exists() && !rel.as_os_str().is_empty() {
                        continue;
                    }
                    let mut children: Vec<PathBuf> = std::fs::read_dir(&abs)?
                        .filter_map(|e| e.ok())
                        .map(|e| rel.join(e.file_name()))
                        .collect();
                    children.sort();
                    stack.extend(children.into_iter().rev());
                }
                Ok(m) => {
                    bytes += m.len();
                    out.push(rel);
                }
                // Missing now: captured as absent.
                Err(_) => out.push(rel),
            }
            if bytes > MAX_SAVED_BYTES || out.len() > MAX_SAVED_FILES {
                return Err(GfError::Unsupported("too much to save for undo"));
            }
        }
        out.sort();
        out.dedup();
        self.files = out;
        Ok(())
    }
}

/// Where HEAD points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    /// Short branch name; `None` when detached.
    pub branch: Option<String>,
    /// `None` on an unborn branch.
    pub oid: Option<String>,
}

/// One index entry (`<mode> <oid> <stage>\t<path>`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct IndexEntry {
    pub path: PathBuf,
    pub stage: u8,
    pub mode: String,
    pub oid: String,
}

/// A work tree file as saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileContent {
    Blob { oid: String, executable: bool },
    Symlink(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashItem {
    pub oid: String,
    pub message: String,
}

/// The state of a [`Scope`] at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capture {
    pub head: Option<Head>,
    pub index: Vec<IndexEntry>,
    /// `None` = the file does not exist.
    pub files: Vec<(PathBuf, Option<FileContent>)>,
    /// `None` = the ref does not exist.
    pub refs: Vec<(String, Option<String>)>,
    /// Newest first, like `git stash list`.
    pub stash: Vec<StashItem>,
    /// `(key, value)` in file order.
    pub config: Vec<(String, String)>,
    pub submodules: Vec<(PathBuf, Option<Head>)>,
}

impl Capture {
    /// The parts that differ from `other`, for telling the user why an
    /// undo would overwrite something. Empty when they match.
    pub fn differences(&self, other: &Self) -> Vec<String> {
        let mut out = Vec::new();
        if self.head != other.head {
            out.push(match &other.head {
                Some(Head {
                    branch: Some(b), ..
                }) => format!("HEAD ({b}) moved"),
                _ => "HEAD moved".into(),
            });
        }
        let changed = |a: &Vec<(PathBuf, Option<FileContent>)>, b: &Vec<_>| -> Vec<PathBuf> {
            a.iter()
                .filter(|(p, c)| b.iter().find(|(q, _)| q == p).map(|(_, d)| d) != Some(c))
                .map(|(p, _)| p.clone())
                .collect()
        };
        for p in changed(&self.files, &other.files) {
            out.push(format!("{} changed", p.display()));
        }
        let mut index_paths: Vec<&PathBuf> = self
            .index
            .iter()
            .chain(&other.index)
            .map(|e| &e.path)
            .collect();
        index_paths.sort();
        index_paths.dedup();
        for p in index_paths {
            let mine: Vec<_> = self.index.iter().filter(|e| &e.path == p).collect();
            let theirs: Vec<_> = other.index.iter().filter(|e| &e.path == p).collect();
            if mine != theirs {
                out.push(format!("{} staged differently", p.display()));
            }
        }
        for (name, id) in &self.refs {
            let now = other.refs.iter().find(|(n, _)| n == name).map(|(_, i)| i);
            if now != Some(id) {
                out.push(format!("{} moved", short_ref(name)));
            }
        }
        if stash_ids(&self.stash) != stash_ids(&other.stash) {
            out.push("stash list changed".into());
        }
        if self.config != other.config {
            out.push("settings changed".into());
        }
        for (p, h) in &self.submodules {
            let now = other
                .submodules
                .iter()
                .find(|(q, _)| q == p)
                .map(|(_, h)| h);
            if now != Some(h) {
                out.push(format!("submodule {} moved", p.display()));
            }
        }
        out
    }

    /// Whether `other` (the repository now) still looks like this capture.
    /// The stash is compared as a set: storing an entry back puts it on top.
    pub fn matches(&self, other: &Self) -> bool {
        self.differences(other).is_empty()
    }
}

fn stash_ids(items: &[StashItem]) -> Vec<&str> {
    let mut ids: Vec<&str> = items.iter().map(|s| s.oid.as_str()).collect();
    ids.sort_unstable();
    ids
}

fn short_ref(name: &str) -> &str {
    name.strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/tags/"))
        .unwrap_or(name)
}

/// How HEAD is moved back when the branch stays the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HeadMode {
    /// `reset --soft`: the index keeps what the commits added (undoing a
    /// commit leaves its changes staged).
    Soft,
    /// `reset --keep`: files follow, uncommitted work is kept (git refuses
    /// rather than overwrite it).
    #[default]
    Keep,
    /// `reset --mixed`: the index follows, files stay (undoing a mixed
    /// reset).
    Mixed,
}

impl CliBackend {
    /// Paths are taken literally (no globbing) by everything here.
    fn literal(&self, args: &[&str]) -> GitCommand {
        self.command(args).env("GIT_LITERAL_PATHSPECS", "1")
    }

    async fn try_read(&self, cmd: GitCommand) -> Option<String> {
        let out = self
            .runner()
            .run(&cmd.read_only().allow_failure())
            .await
            .ok()?;
        out.success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
    }

    async fn head_at(&self, dir: &Path) -> Option<Head> {
        let branch = self
            .try_read(
                self.command(["symbolic-ref", "-q", "--short", "HEAD"])
                    .current_dir(dir),
            )
            .await
            .filter(|s| !s.is_empty());
        let oid = self
            .try_read(
                self.command(["rev-parse", "-q", "--verify", "HEAD"])
                    .current_dir(dir),
            )
            .await
            .filter(|s| !s.is_empty());
        (branch.is_some() || oid.is_some()).then_some(Head { branch, oid })
    }

    async fn capture_index(&self, paths: &[PathBuf]) -> Result<Vec<IndexEntry>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let out = self
            .read(self.literal(&["ls-files", "-s", "-z"]).paths(paths))
            .await?;
        let mut entries: Vec<IndexEntry> = out
            .split(|b| *b == 0)
            .filter_map(|rec| {
                let rec = std::str::from_utf8(rec).ok()?;
                let (meta, path) = rec.split_once('\t')?;
                let mut f = meta.split(' ');
                Some(IndexEntry {
                    mode: f.next()?.to_owned(),
                    oid: f.next()?.to_owned(),
                    stage: f.next()?.parse().ok()?,
                    path: PathBuf::from(path),
                })
            })
            .collect();
        entries.sort();
        Ok(entries)
    }

    async fn capture_files(
        &self,
        paths: &[PathBuf],
    ) -> Result<Vec<(PathBuf, Option<FileContent>)>> {
        let mut out: Vec<(PathBuf, Option<FileContent>)> = Vec::new();
        let mut to_hash = Vec::new();
        for rel in paths {
            let abs = self.cwd().join(rel);
            match std::fs::symlink_metadata(&abs) {
                Ok(m) if m.file_type().is_symlink() => {
                    out.push((
                        rel.clone(),
                        Some(FileContent::Symlink(std::fs::read_link(&abs)?)),
                    ));
                }
                Ok(m) if m.is_file() && needs_own_hash(rel) => {
                    let id = self
                        .read(
                            self.command(["hash-object", "-w", "--no-filters", "--"])
                                .arg(rel),
                        )
                        .await?;
                    let oid = String::from_utf8_lossy(&id).trim_end().to_owned();
                    out.push((
                        rel.clone(),
                        Some(FileContent::Blob {
                            oid,
                            executable: executable(&m),
                        }),
                    ));
                }
                Ok(m) if m.is_file() => {
                    to_hash.push((out.len(), executable(&m)));
                    out.push((rel.clone(), None));
                }
                _ => out.push((rel.clone(), None)),
            }
        }
        if !to_hash.is_empty() {
            let input: String = to_hash
                .iter()
                .map(|(i, _)| format!("{}\n", out[*i].0.display()))
                .collect();
            let ids = self
                .read(
                    self.command(["hash-object", "-w", "--no-filters", "--stdin-paths"])
                        .stdin(input),
                )
                .await?;
            let ids: Vec<String> = String::from_utf8_lossy(&ids)
                .lines()
                .map(str::to_owned)
                .collect();
            if ids.len() != to_hash.len() {
                return Err(GfError::Other("hash-object returned too few ids".into()));
            }
            for ((i, exec), oid) in to_hash.into_iter().zip(ids) {
                out[i].1 = Some(FileContent::Blob {
                    oid,
                    executable: exec,
                });
            }
        }
        Ok(out)
    }

    async fn capture_refs(&self, names: &[String]) -> Result<Vec<(String, Option<String>)>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let out = self
            .read(
                self.command(["for-each-ref", "--format=%(refname)%00%(objectname)"])
                    .args(names),
            )
            .await?;
        let text = String::from_utf8_lossy(&out);
        let found: Vec<(&str, &str)> = text.lines().filter_map(|l| l.split_once('\0')).collect();
        Ok(names
            .iter()
            .map(|n| {
                let id = found
                    .iter()
                    .find(|(r, _)| r == n)
                    .map(|(_, id)| (*id).to_owned());
                (n.clone(), id)
            })
            .collect())
    }

    async fn capture_stash(&self) -> Vec<StashItem> {
        self.try_read(self.command(["stash", "list", "--format=%H%x1f%gs"]))
            .await
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let (oid, message) = l.split_once('\x1f')?;
                Some(StashItem {
                    oid: oid.to_owned(),
                    message: message.to_owned(),
                })
            })
            .collect()
    }

    async fn capture_config(&self, sections: &[String]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for section in sections {
            let pattern = format!("^{}\\.", regex_escape(section));
            let text = self
                .try_read(
                    self.command(["config", "--local", "-z", "--get-regexp"])
                        .arg(pattern),
                )
                .await
                .unwrap_or_default();
            for rec in text.split('\0').filter(|r| !r.is_empty()) {
                // A key with no `=` at all means true (and can only be
                // written back with a value).
                let (k, v) = rec.split_once('\n').unwrap_or((rec, "true"));
                // `^branch\.x\.` also matches `branch.x.y.remote` (branch
                // `x.y`): keep only this section's own keys.
                if in_section(k, section) {
                    out.push((k.to_owned(), v.to_owned()));
                }
            }
        }
        out
    }
}

impl UndoOps for CliBackend {
    async fn stash_paths(&self, index: usize) -> Result<Vec<PathBuf>> {
        let stash = format!("stash@{{{index}}}");
        let mut paths: Vec<PathBuf> = Vec::new();
        let tracked = self
            .read(
                self.command(["diff", "--name-only", "-z", "--no-renames"])
                    .args([format!("{stash}^1"), stash.clone()]),
            )
            .await?;
        let staged = self
            .read(
                self.command(["diff", "--name-only", "-z", "--no-renames"])
                    .args([format!("{stash}^1"), format!("{stash}^2")]),
            )
            .await?;
        let untracked = self
            .try_read(
                self.command(["ls-tree", "-r", "-z", "--name-only"])
                    .arg(format!("{stash}^3")),
            )
            .await
            .unwrap_or_default();
        for chunk in [tracked, staged, untracked.into_bytes()] {
            paths.extend(
                chunk
                    .split(|b| *b == 0)
                    .filter(|p| !p.is_empty())
                    .map(|p| PathBuf::from(String::from_utf8_lossy(p).into_owned())),
            );
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    async fn capture(&self, scope: &Scope) -> Result<Capture> {
        let head = if scope.head {
            self.head_at(self.cwd()).await
        } else {
            None
        };
        let mut submodules = Vec::new();
        for p in &scope.submodules {
            let dir = self.cwd().join(p);
            let head = if dir.join(".git").exists() {
                self.head_at(&dir).await
            } else {
                None
            };
            submodules.push((p.clone(), head));
        }
        Ok(Capture {
            head,
            index: self.capture_index(&scope.index).await?,
            files: self.capture_files(&scope.files).await?,
            refs: self.capture_refs(&scope.refs).await?,
            stash: if scope.stash {
                self.capture_stash().await
            } else {
                Vec::new()
            },
            config: self.capture_config(&scope.config).await,
            submodules,
        })
    }

    async fn restore(
        &self,
        scope: &Scope,
        expected: Option<&Capture>,
        to: &Capture,
        mode: HeadMode,
    ) -> Result<()> {
        let now = self.capture(scope).await?;

        // Checked again right before writing: something may have changed
        // while the user was reading the confirmation.
        if let Some(expected) = expected {
            let changed = expected.differences(&now);
            if !changed.is_empty() {
                return Err(GfError::Other(format!(
                    "changed since it was checked ({}) — nothing was changed, try again",
                    changed.join(", ")
                )));
            }
        }

        // Everything needed is still there before anything is changed:
        // saved file contents can be pruned by `git gc` after two weeks.
        let blobs: Vec<&str> = to
            .files
            .iter()
            .filter_map(|(_, c)| match c {
                Some(FileContent::Blob { oid, .. }) => Some(oid.as_str()),
                _ => None,
            })
            .collect();
        if !blobs.is_empty() {
            let input: String = blobs.iter().map(|o| format!("{o}\n")).collect();
            let out = self
                .read(
                    self.command(["cat-file", "--batch-check=%(objecttype)"])
                        .stdin(input),
                )
                .await?;
            let missing = String::from_utf8_lossy(&out)
                .lines()
                .filter(|l| *l != "blob")
                .count();
            if missing > 0 {
                return Err(GfError::Other(format!(
                    "the saved copy of {missing} file(s) is gone (git cleans it up after about two weeks) — nothing was changed"
                )));
            }
        }

        // Refs that exist in the target are set before HEAD may point at them.
        for (name, id) in &to.refs {
            let cur = now
                .refs
                .iter()
                .find(|(n, _)| n == name)
                .and_then(|(_, i)| i.as_ref());
            if let Some(id) = id {
                if cur != Some(id) {
                    self.exec(
                        self.command(["update-ref", "-m", "gf: undo"])
                            .args([name, id]),
                    )
                    .await?;
                }
            }
        }

        if scope.head {
            if let Some(target) = &to.head {
                self.restore_head(now.head.as_ref(), target, mode, &scope.files)
                    .await?;
            }
        }

        for (name, id) in &to.refs {
            let exists = now.refs.iter().any(|(n, i)| n == name && i.is_some());
            if id.is_none() && exists {
                self.exec(
                    self.command(["update-ref", "-m", "gf: undo", "-d"])
                        .arg(name),
                )
                .await?;
            }
        }

        for section in &scope.config {
            let _ = self
                .runner()
                .run(
                    &self
                        .command(["config", "--local", "--remove-section"])
                        .arg(section)
                        .allow_failure(),
                )
                .await;
        }
        for (k, v) in &to.config {
            self.exec(self.command(["config", "--local", "--add"]).args([k, v]))
                .await?;
        }

        if !scope.index.is_empty() {
            let zero = "0".repeat(
                to.index
                    .first()
                    .or(now.index.first())
                    .map_or(40, |e| e.oid.len()),
            );
            let mut input = Vec::new();
            for p in &scope.index {
                input.extend(format!("0 {zero}\t{}\0", p.display()).into_bytes());
            }
            for e in &to.index {
                input.extend(
                    format!("{} {} {}\t{}\0", e.mode, e.oid, e.stage, e.path.display())
                        .into_bytes(),
                );
            }
            self.exec(
                self.command(["update-index", "-z", "--index-info"])
                    .stdin(input),
            )
            .await?;
        }

        for (rel, content) in &to.files {
            let abs = self.cwd().join(rel);
            match content {
                None => {
                    if std::fs::symlink_metadata(&abs).is_ok_and(|m| !m.is_dir()) {
                        std::fs::remove_file(&abs)?;
                        remove_empty_parents(self.cwd(), &abs);
                    }
                }
                Some(FileContent::Blob { oid, executable }) => {
                    let bytes = self.read(self.command(["cat-file", "blob", oid])).await?;
                    write_file(&abs, &bytes, *executable)?;
                }
                Some(FileContent::Symlink(target)) => {
                    if let Some(dir) = abs.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    let _ = std::fs::remove_file(&abs);
                    symlink(target, &abs)?;
                }
            }
        }

        if scope.stash {
            // Entries not in the target are dropped last, once the files
            // they hold are back (undoing a stash push drops its entry).
            let extra: Vec<usize> = now
                .stash
                .iter()
                .enumerate()
                .filter(|(_, s)| !to.stash.iter().any(|t| t.oid == s.oid))
                .map(|(i, _)| i)
                .collect();
            for i in extra.into_iter().rev() {
                self.exec(
                    self.command(["stash", "drop", "-q"])
                        .arg(format!("stash@{{{i}}}")),
                )
                .await?;
            }
            // Oldest first, so the newest missing entry ends up on top.
            for s in to.stash.iter().rev() {
                if !now.stash.iter().any(|n| n.oid == s.oid) {
                    self.exec(
                        self.command(["stash", "store", "-m"])
                            .args([&s.message, &s.oid]),
                    )
                    .await?;
                }
            }
        }

        for (p, head) in &to.submodules {
            let Some(Head { branch, oid }) = head else {
                continue;
            };
            let dir = self.cwd().join(p);
            let in_sub = |args: &[&str]| self.command(args).current_dir(&dir);
            if let Some(b) = branch {
                self.exec(in_sub(&["checkout", "-q"]).arg(b)).await?;
            }
            if let Some(oid) = oid {
                let at = self.try_read(in_sub(&["rev-parse", "HEAD"])).await;
                if at.as_deref() != Some(oid.as_str()) {
                    let cmd = if branch.is_some() {
                        in_sub(&["reset", "-q", "--keep"])
                    } else {
                        in_sub(&["checkout", "-q", "--detach"])
                    };
                    self.exec(cmd.arg(oid)).await?;
                }
            }
        }
        Ok(())
    }
}

impl CliBackend {
    /// Moves HEAD to `to`. `owned` are the files this step restores right
    /// after: changes in them may be overwritten by a switch (redoing
    /// "stash, then switch" finds them back while still on the old branch).
    /// Whether there are changes and all of them are in `owned` (files or
    /// folders): then nothing outside the step would be lost by forcing.
    async fn only_changed_in(&self, owned: &[PathBuf]) -> Result<bool> {
        if owned.is_empty() {
            return Ok(false);
        }
        let out = self
            .read(self.command([
                "status",
                "--porcelain",
                "-z",
                "--untracked-files=all",
                "--no-renames",
            ]))
            .await?;
        let changed: Vec<PathBuf> = out
            .split(|b| *b == 0)
            .filter(|e| e.len() > 3)
            .map(|e| PathBuf::from(String::from_utf8_lossy(&e[3..]).into_owned()))
            .collect();
        Ok(!changed.is_empty()
            && changed
                .iter()
                .all(|c| owned.iter().any(|o| c.starts_with(o))))
    }

    async fn restore_head(
        &self,
        now: Option<&Head>,
        to: &Head,
        mode: HeadMode,
        owned: &[PathBuf],
    ) -> Result<()> {
        if now == Some(to) {
            return Ok(());
        }
        let now_oid = now.and_then(|h| h.oid.as_deref());
        let now_branch = now.and_then(|h| h.branch.as_deref());
        let reset = |oid: &str| {
            let flag = match mode {
                HeadMode::Soft => "--soft",
                HeadMode::Keep => "--keep",
                HeadMode::Mixed => "--mixed",
            };
            self.command(["reset", "-q", flag, "--end-of-options", oid])
        };
        match (&to.branch, &to.oid) {
            // Unborn: undoing the first commit removes the branch it made
            // (the index keeps the files staged).
            (Some(b), None) => {
                if now_branch == Some(b.as_str()) && now_oid.is_some() {
                    self.exec(
                        self.command(["update-ref", "-m", "gf: undo", "-d"])
                            .arg(format!("refs/heads/{b}")),
                    )
                    .await?;
                }
            }
            // Back on the same branch: move it (commit, merge, pull…).
            (Some(b), Some(oid)) if now_branch == Some(b.as_str()) => {
                self.exec(reset(oid)).await?;
            }
            // Another branch at the same commit (rename, or a checkout that
            // did not change files): just point HEAD at it.
            (Some(b), Some(oid)) if now_oid == Some(oid.as_str()) => {
                self.exec(
                    self.command(["symbolic-ref", "-m", "gf: undo", "HEAD"])
                        .arg(format!("refs/heads/{b}")),
                )
                .await?;
            }
            (Some(b), oid) => {
                if self.only_changed_in(owned).await? {
                    self.exec(self.command(["switch", "-q", "--discard-changes"]).arg(b))
                        .await?;
                } else {
                    self.exec(self.command(["switch", "-q"]).arg(b)).await?;
                }
                if let Some(oid) = oid {
                    let at = self.try_read(self.command(["rev-parse", "HEAD"])).await;
                    if at.as_deref() != Some(oid.as_str()) {
                        self.exec(reset(oid)).await?;
                    }
                }
            }
            // Still detached: move HEAD the step's way (undoing a commit on
            // a detached HEAD keeps its changes staged, like on a branch).
            (None, Some(oid)) if now.is_some_and(|h| h.branch.is_none() && h.oid.is_some()) => {
                self.exec(reset(oid)).await?;
            }
            (None, Some(oid)) => {
                self.exec(
                    self.command(["switch", "-q", "--detach", "--end-of-options"])
                        .arg(oid),
                )
                .await?;
            }
            (None, None) => {}
        }
        Ok(())
    }
}

/// Whether config `key` (`section[.subsection].name`) belongs to exactly
/// `section` (`branch.x`). Section names compare without case, like git.
fn in_section(key: &str, section: &str) -> bool {
    let Some((own, _name)) = key.rsplit_once('.') else {
        return false;
    };
    match (own.split_once('.'), section.split_once('.')) {
        (Some((a, sub_a)), Some((b, sub_b))) => a.eq_ignore_ascii_case(b) && sub_a == sub_b,
        (None, None) => own.eq_ignore_ascii_case(section),
        _ => false,
    }
}

/// Paths `hash-object --stdin-paths` would misread (one path per line, a
/// leading `"` is unquoted): hashed one at a time instead.
fn needs_own_hash(p: &Path) -> bool {
    p.to_str()
        .is_none_or(|s| s.contains(['\n', '\r']) || s.starts_with('"'))
}

fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r"\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(unix)]
fn executable(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &std::fs::Metadata) -> bool {
    false
}

fn write_file(abs: &Path, bytes: &[u8], exec: bool) -> Result<()> {
    if let Some(dir) = abs.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::symlink_metadata(abs).is_ok_and(|m| m.file_type().is_symlink()) {
        std::fs::remove_file(abs)?;
    }
    std::fs::write(abs, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(abs)?.permissions();
        let mode = perm.mode();
        perm.set_mode(if exec { mode | 0o111 } else { mode & !0o111 });
        std::fs::set_permissions(abs, perm)?;
    }
    #[cfg(not(unix))]
    let _ = exec;
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, at: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, at)?;
    Ok(())
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _at: &Path) -> Result<()> {
    Err(GfError::Unsupported("restoring symlinks"))
}

/// Removes directories left empty by deleting `file`, up to `root`.
fn remove_empty_parents(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_config_sections() {
        assert_eq!(regex_escape("branch.feat/x+y"), r"branch\.feat/x\+y");
    }

    #[test]
    fn differences_name_what_changed() {
        let a = Capture {
            files: vec![("a.txt".into(), None)],
            refs: vec![("refs/heads/x".into(), Some("1".into()))],
            ..Capture::default()
        };
        let mut b = a.clone();
        assert!(a.matches(&b));
        b.files[0].1 = Some(FileContent::Blob {
            oid: "2".into(),
            executable: false,
        });
        b.refs[0].1 = None;
        assert_eq!(a.differences(&b), ["a.txt changed", "x moved"]);
    }

    #[test]
    fn stash_is_compared_as_a_set() {
        let item = |o: &str| StashItem {
            oid: o.into(),
            message: String::new(),
        };
        let a = Capture {
            stash: vec![item("1"), item("2")],
            ..Capture::default()
        };
        let b = Capture {
            stash: vec![item("2"), item("1")],
            ..Capture::default()
        };
        assert!(a.matches(&b));
    }

    #[test]
    fn expanding_lists_files_under_folders_and_keeps_missing_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("d/e")).expect("mkdir");
        std::fs::write(dir.path().join("d/one"), "1").expect("write");
        std::fs::write(dir.path().join("d/e/two"), "2").expect("write");
        let mut s = Scope {
            files: vec!["d".into(), "gone".into()],
            ..Scope::default()
        };
        s.expand_files(dir.path()).expect("expand");
        assert_eq!(
            s.files,
            [PathBuf::from("d/e/two"), "d/one".into(), "gone".into()]
        );
    }
}
