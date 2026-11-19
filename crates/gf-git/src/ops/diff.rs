use std::ffi::OsString;
use std::path::{Path, PathBuf};

use gf_core::Result;
use gf_core::model::{FileDiff, Oid};

use crate::backend::{CliBackend, DiffOps};
use crate::cli::parse::diff::{DIFF_FORMAT_ARGS, parse_diff};

/// Which version supplies the contents of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileVersion {
    WorkTree,
    Index,
    Revision(String),
}

/// Which two states to compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffTarget {
    /// Unstaged changes: index → working tree.
    WorkTree,
    /// Staged changes: HEAD → index.
    Staged,
    /// A commit against its first parent (root commits against nothing).
    Commit(Oid),
    /// Any two revisions (`git diff <from> <to>`).
    Range { from: String, to: String },
    /// An untracked file shown as entirely added (`git diff --no-index`).
    Untracked(PathBuf),
}

#[derive(Debug, Clone, Default)]
pub struct DiffOptions {
    /// Lines of context (`-U<n>`); git's default (3) when `None`.
    pub context_lines: Option<u32>,
    /// `--ignore-all-space`.
    pub ignore_whitespace: bool,
    /// Limit to these paths.
    pub paths: Vec<PathBuf>,
}

impl DiffOps for CliBackend {
    async fn file_bytes(&self, path: &Path, version: &FileVersion) -> Result<Vec<u8>> {
        if matches!(version, FileVersion::WorkTree) {
            if path.as_os_str().is_empty()
                || !path
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err(gf_core::GfError::Other(
                    "expected a repository-relative file path".into(),
                ));
            }
            let full = self.cwd().join(path);
            let kind = tokio::fs::symlink_metadata(&full).await?.file_type();
            if kind.is_symlink() {
                return Ok(tokio::fs::read_link(full)
                    .await?
                    .into_os_string()
                    .into_encoded_bytes());
            }
            if !kind.is_file() {
                return Err(gf_core::GfError::Unsupported(
                    "preview requires a regular file",
                ));
            }
            return Ok(tokio::fs::read(full).await?);
        }
        let mut object = if let FileVersion::Revision(rev) = version {
            let mut object = OsString::from(rev);
            object.push(":");
            object
        } else {
            // An explicit stage keeps a path starting with `1:` from
            // being interpreted as an index stage selector.
            OsString::from(":0:")
        };
        object.push(path);
        self.read(self.command(["cat-file", "blob", "--"]).arg(object))
            .await
    }

    async fn diff(&self, target: &DiffTarget, opts: &DiffOptions) -> Result<Vec<FileDiff>> {
        if let DiffTarget::Untracked(path) = target {
            return self.diff_untracked(path).await;
        }
        let mut args: Vec<String> = match target {
            DiffTarget::WorkTree => vec!["diff".into()],
            DiffTarget::Staged => vec!["diff".into(), "--cached".into()],
            DiffTarget::Commit(_) => vec![
                "show".into(),
                "--format=".into(),
                "--diff-merges=first-parent".into(),
            ],
            DiffTarget::Range { .. } | DiffTarget::Untracked(_) => vec!["diff".into()],
        };
        args.extend(DIFF_FORMAT_ARGS.iter().map(|s| (*s).to_owned()));
        if let Some(n) = opts.context_lines {
            args.push(format!("-U{n}"));
        }
        if opts.ignore_whitespace {
            args.push("--ignore-all-space".into());
        }
        match target {
            DiffTarget::Commit(oid) => {
                args.push("--end-of-options".into());
                args.push(oid.to_string());
            }
            DiffTarget::Range { from, to } => {
                args.push("--end-of-options".into());
                args.push(from.clone());
                args.push(to.clone());
            }
            DiffTarget::WorkTree | DiffTarget::Staged | DiffTarget::Untracked(_) => {}
        }
        let out = self.read(self.command(args).paths(&opts.paths)).await?;
        parse_diff(&out)
    }

    async fn numstat(
        &self,
        staged: bool,
    ) -> Result<std::collections::HashMap<PathBuf, Option<crate::cli::parse::numstat::LineCounts>>>
    {
        use crate::cli::parse::numstat::{NUMSTAT_ARGS, parse_numstat};
        let mut cmd = self.command(NUMSTAT_ARGS);
        if staged {
            cmd = cmd.arg("--cached");
        }
        parse_numstat(&self.read(cmd).await?)
    }
}

impl CliBackend {
    async fn diff_untracked(&self, path: &std::path::Path) -> Result<Vec<FileDiff>> {
        let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let mut args: Vec<String> = vec!["diff".into(), "--no-index".into()];
        args.extend(DIFF_FORMAT_ARGS.iter().map(|s| (*s).to_owned()));
        args.extend(["--".into(), null.into()]);
        let cmd = self.command(args).arg(path).read_only().allow_failure();
        let out = self.runner().run(&cmd).await?;
        // `--no-index` exits 1 when the files differ, which is the normal case.
        if out.exit_code != Some(0) && out.exit_code != Some(1) {
            return Err(gf_core::GfError::GitCommand {
                argv: vec!["diff".into(), "--no-index".into()],
                exit_code: out.exit_code,
                stderr: out.stderr.trim().to_owned(),
            });
        }
        let mut files = parse_diff(&out.stdout)?;
        for f in &mut files {
            // Report the path relative to the repository like other diffs.
            f.new_path = Some(path.to_path_buf());
        }
        Ok(files)
    }
}
