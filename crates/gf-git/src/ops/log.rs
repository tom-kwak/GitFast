use std::path::PathBuf;

use gf_core::Result;
use gf_core::model::{Commit, Oid};

use crate::backend::{CliBackend, LogOps};
use crate::cli::parse::log::{LOG_ARGS, parse_log};
use crate::cli::parse::{bytes_to_path, split_records};

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

impl LogQuery {
    /// `git rev-list` arguments walking the same commits in the same order
    /// as this query's `git log` (ids only, no limit).
    fn rev_list_args(&self) -> Vec<String> {
        let mut args = vec!["rev-list".to_owned()];
        match self.order {
            LogOrder::Default => {}
            LogOrder::Topo => args.push("--topo-order".into()),
            LogOrder::Date => args.push("--date-order".into()),
        }
        if self.first_parent {
            args.push("--first-parent".into());
        }
        if self.all_refs {
            args.push("--all".into());
        }
        args.push("--end-of-options".into());
        if self.revisions.is_empty() && !self.all_refs {
            // `git log` walks HEAD by default; `rev-list` needs it said.
            args.push("HEAD".into());
        }
        args.extend(self.revisions.iter().cloned());
        args
    }
}

fn parse_tree_paths(out: &[u8]) -> Vec<PathBuf> {
    split_records(out, 0).map(bytes_to_path).collect()
}

impl CliBackend {
    /// Paths present in the tree of `commit`, preserving native path bytes.
    pub(crate) async fn commit_file_paths(&self, commit: &Oid) -> Result<Vec<PathBuf>> {
        let out = self
            .read(self.command(["ls-tree", "-r", "-z", "--name-only", commit.as_str()]))
            .await?;
        Ok(parse_tree_paths(&out))
    }

    /// Row of `oid` in `query`'s history (0 = first), reading ids only and
    /// stopping at it; `None` when the walk never reaches it. The query's
    /// limit, skip and filters are ignored: plain histories only.
    pub(crate) async fn log_position(&self, query: &LogQuery, oid: &str) -> Result<Option<usize>> {
        let cmd = self.command(query.rev_list_args()).read_only();
        let opts = crate::cli::RunOptions {
            stop_at: Some(oid.as_bytes().to_vec()),
            ..crate::cli::RunOptions::default()
        };
        let out = self.runner().run_with(&cmd, opts).await?;
        Ok(out
            .stdout
            .split(|b| *b == b'\n')
            .position(|line| line == oid.as_bytes()))
    }
}

impl CliBackend {
    /// The newest commit where `--follow` crossed a *copy* (the file appeared
    /// as a copy of one that still exists, e.g. `AGENTS.md` next to
    /// `CLAUDE.md`). Only renames carry a file's history; a copy's source
    /// history belongs to another file.
    async fn follow_copy_boundary(&self, query: &LogQuery) -> Result<Option<String>> {
        let mut args: Vec<String> = vec![
            "log".into(),
            "--follow".into(),
            "--diff-filter=C".into(),
            "--name-status".into(),
            "--format=%H".into(),
        ];
        if query.first_parent {
            args.push("--first-parent".into());
        }
        args.push("--end-of-options".into());
        args.extend(query.revisions.iter().cloned());
        let cmd = self
            .command(args)
            .paths(&query.paths)
            .env("GIT_LITERAL_PATHSPECS", "1")
            .read_only()
            .allow_failure();
        let out = self.runner().run(&cmd).await?;
        if !out.success() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text
            .lines()
            .find(|l| l.len() >= 40 && l.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(str::to_owned))
    }
}

impl LogOps for CliBackend {
    async fn log(&self, query: &LogQuery) -> Result<Vec<Commit>> {
        if query.follow
            && query.paths.len() == 1
            && let Some(boundary) = self.follow_copy_boundary(query).await?
        {
            {
                // Walk everything, cut at the copy, then page.
                let mut all = LogQuery {
                    max_count: None,
                    skip: 0,
                    ..query.clone()
                };
                all.follow = true;
                let mut commits = self.log_raw(&all).await?;
                if let Some(i) = commits.iter().position(|c| c.id.as_str() == boundary) {
                    commits.truncate(i + 1);
                }
                let rest = commits.into_iter().skip(query.skip);
                return Ok(match query.max_count {
                    Some(n) => rest.take(n).collect(),
                    None => rest.collect(),
                });
            }
        }
        self.log_raw(query).await
    }

    async fn head_exists(&self) -> Result<bool> {
        let cmd = self
            .command(["rev-parse", "--verify", "--quiet", "HEAD"])
            .read_only()
            .allow_failure();
        Ok(self.runner().run(&cmd).await?.success())
    }
}

impl CliBackend {
    async fn log_raw(&self, query: &LogQuery) -> Result<Vec<Commit>> {
        let cmd = self
            .command(query.to_args())
            .paths(&query.paths)
            .read_only()
            .allow_failure();
        // Follow the selected file literally, even if its name contains Git pathspec magic.
        let cmd = if query.follow && query.paths.len() == 1 {
            cmd.env("GIT_LITERAL_PATHSPECS", "1")
        } else {
            cmd
        };
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn tree_path_records_preserve_non_utf8_bytes() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let paths = parse_tree_paths(b"line\nbreak.txt\0non-utf8-\xff.txt\0");
        assert_eq!(paths[0], PathBuf::from("line\nbreak.txt"));
        assert_eq!(
            paths[1],
            PathBuf::from(OsStr::from_bytes(b"non-utf8-\xff.txt"))
        );
    }

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
