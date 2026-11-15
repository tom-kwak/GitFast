use std::path::PathBuf;

use gf_core::Result;
use gf_core::model::{FileDiff, Oid};

use crate::Repository;
use crate::cli::parse::diff::{DIFF_FORMAT_ARGS, parse_diff};

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

impl Repository {
    pub async fn diff(&self, target: &DiffTarget, opts: &DiffOptions) -> Result<Vec<FileDiff>> {
        let mut args: Vec<String> = match target {
            DiffTarget::WorkTree => vec!["diff".into()],
            DiffTarget::Staged => vec!["diff".into(), "--cached".into()],
            DiffTarget::Commit(_) => vec![
                "show".into(),
                "--format=".into(),
                "--diff-merges=first-parent".into(),
            ],
            DiffTarget::Range { .. } => vec!["diff".into()],
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
            DiffTarget::WorkTree | DiffTarget::Staged => {}
        }
        let out = self.read(self.command(args).paths(&opts.paths)).await?;
        parse_diff(&out)
    }
}
