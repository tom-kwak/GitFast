use std::path::PathBuf;

use gf_core::Result;
use gf_core::model::Commit;

use crate::backend::{CliBackend, LogOps};
use crate::cli::parse::log::{LOG_ARGS, parse_log};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogOrder {
    /// git's default: reverse chronological by commit date while walking.
    #[default]
    Default,
    /// `--topo-order`: never shows a parent before all its children. Used by
    /// the commit graph.
    Topo,
    /// `--date-order`.
    Date,
}

/// What `git log` should list. The default lists everything reachable from
/// HEAD.
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Revisions/ranges to start from (`main`, `v1..v2`). Empty means HEAD.
    pub revisions: Vec<String>,
    /// `--all`: start from every ref.
    pub all_refs: bool,
    pub max_count: Option<usize>,
    pub skip: usize,
    pub order: LogOrder,
    pub first_parent: bool,
    /// Only commits touching these paths.
    pub paths: Vec<PathBuf>,
    /// `--follow`: track a single file across renames.
    pub follow: bool,
    /// `--author=<pattern>`.
    pub author: Option<String>,
    /// `--grep=<pattern>` on the message (case-insensitive).
    pub grep: Option<String>,
    /// `-S<string>`: commits that change the number of occurrences.
    pub pickaxe: Option<String>,
}

impl LogQuery {
    pub fn head(max_count: usize) -> Self {
        Self {
            max_count: Some(max_count),
            ..Self::default()
        }
    }

    /// True when only revision walking from plain revisions is needed (no
    /// filters, ranges or ordering options).
    pub fn is_plain_walk(&self) -> bool {
        self.order == LogOrder::Default
            && !self.all_refs
            && !self.first_parent
            && self.paths.is_empty()
            && self.author.is_none()
            && self.grep.is_none()
            && self.pickaxe.is_none()
            // Plain revisions only: no ranges (`a..b`) or exclusions (`^a`).
            && self
                .revisions
                .iter()
                .all(|r| !r.contains("..") && !r.starts_with(['^', '-']))
    }

    fn to_args(&self) -> Vec<String> {
        let mut args: Vec<String> = LOG_ARGS.iter().map(|s| (*s).to_owned()).collect();
        match self.order {
            LogOrder::Default => {}
            LogOrder::Topo => args.push("--topo-order".into()),
            LogOrder::Date => args.push("--date-order".into()),
        }
        if let Some(n) = self.max_count {
            args.push(format!("--max-count={n}"));
        }
        if self.skip > 0 {
            args.push(format!("--skip={}", self.skip));
        }
        if self.first_parent {
            args.push("--first-parent".into());
        }
        if self.follow && self.paths.len() == 1 {
            args.push("--follow".into());
        }
        if let Some(a) = &self.author {
            args.push(format!("--author={a}"));
        }
        if let Some(g) = &self.grep {
            args.push(format!("--grep={g}"));
            args.push("--regexp-ignore-case".into());
        }
        if let Some(s) = &self.pickaxe {
            args.push(format!("-S{s}"));
        }
        if self.all_refs {
            args.push("--all".into());
        }
        // `--end-of-options` stops a revision like `-x` from being read as a flag.
        args.push("--end-of-options".into());
        args.extend(self.revisions.iter().cloned());
        args
    }
}

impl LogOps for CliBackend {
    async fn log(&self, query: &LogQuery) -> Result<Vec<Commit>> {
        let cmd = self
            .command(query.to_args())
            .paths(&query.paths)
            .read_only()
            .allow_failure();
        let out = self.runner().run(&cmd).await?;
        if !out.success() {
            // An unborn branch (no commits yet) is an empty history, not an
            // error. Checked via rev-parse rather than git's (localized) message.
            let walks_head = query.revisions.is_empty() && !query.all_refs;
            if walks_head && !self.head_exists().await? {
                return Ok(Vec::new());
            }
            return Err(gf_core::GfError::GitCommand {
                argv: cmd
                    .user_args()
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect(),
                exit_code: out.exit_code,
                stderr: out.stderr.trim().to_owned(),
            });
        }
        parse_log(&out.stdout)
    }

    async fn head_exists(&self) -> Result<bool> {
        let cmd = self
            .command(["rev-parse", "--verify", "--quiet", "HEAD"])
            .read_only()
            .allow_failure();
        Ok(self.runner().run(&cmd).await?.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_filtered_args() {
        let q = LogQuery {
            revisions: vec!["main".into()],
            max_count: Some(50),
            skip: 10,
            order: LogOrder::Topo,
            author: Some("kim".into()),
            grep: Some("fix".into()),
            ..LogQuery::default()
        };
        let args = q.to_args();
        for expected in [
            "--topo-order",
            "--max-count=50",
            "--skip=10",
            "--author=kim",
            "--grep=fix",
        ] {
            assert!(args.iter().any(|a| a == expected), "missing {expected}");
        }
        assert_eq!(args.last().map(String::as_str), Some("main"));
        assert!(!q.is_plain_walk());
        assert!(LogQuery::head(100).is_plain_walk());
    }
}
