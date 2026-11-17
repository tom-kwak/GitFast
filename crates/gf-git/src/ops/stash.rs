//! Stash: list, save, apply, pop, drop.

use gf_core::model::{StashEntry, Timestamp};
use gf_core::{GfError, Result};

use crate::backend::{CliBackend, StashOps};

#[derive(Debug, Clone, Default)]
pub struct StashOptions {
    pub message: Option<String>,
    /// Also stash untracked files.
    pub include_untracked: bool,
    /// Leave staged changes in the index.
    pub keep_index: bool,
    /// Only these paths (all changes when empty).
    pub paths: Vec<std::path::PathBuf>,
}

/// `%gd` reflog selector, hash, subject, committer time (unix seconds).
/// No `--date` option: it would also change `%gd` into a date selector.
const STASH_FORMAT: &str = "--format=%gd%x1f%H%x1f%gs%x1f%ct";

pub fn parse_stash_list(out: &[u8]) -> Result<Vec<StashEntry>> {
    String::from_utf8_lossy(out)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let bad = || GfError::parse("stash list", line.to_owned());
            let f: Vec<&str> = line.splitn(4, '\x1f').collect();
            let [selector, id, message, seconds] = f.as_slice() else {
                return Err(bad());
            };
            let index = selector
                .strip_prefix("stash@{")
                .and_then(|s| s.strip_suffix('}'))
                .and_then(|n| n.parse().ok())
                .ok_or_else(bad)?;
            Ok(StashEntry {
                index,
                id: id.parse()?,
                message: (*message).to_owned(),
                time: Timestamp {
                    seconds: seconds.trim().parse().map_err(|_| bad())?,
                    offset_minutes: 0,
                },
            })
        })
        .collect()
}

impl StashOps for CliBackend {
    async fn stashes(&self) -> Result<Vec<StashEntry>> {
        let out = self
            .read(self.command(["stash", "list", STASH_FORMAT]))
            .await?;
        parse_stash_list(&out)
    }

    async fn stash_push(&self, opts: &StashOptions) -> Result<()> {
        let mut cmd = self.command(["stash", "push"]);
        if opts.include_untracked {
            cmd = cmd.arg("--include-untracked");
        }
        if opts.keep_index {
            cmd = cmd.arg("--keep-index");
        }
        if let Some(m) = opts.message.as_deref().filter(|m| !m.trim().is_empty()) {
            cmd = cmd.args(["-m", m]);
        }
        self.exec(cmd.paths(&opts.paths)).await?;
        Ok(())
    }

    async fn stash_apply(&self, index: usize) -> Result<()> {
        self.exec(self.command(["stash", "apply", "--index", &format!("stash@{{{index}}}")]))
            .await?;
        Ok(())
    }

    async fn stash_pop(&self, index: usize) -> Result<()> {
        self.exec(self.command(["stash", "pop", "--index", &format!("stash@{{{index}}}")]))
            .await?;
        Ok(())
    }

    async fn stash_drop(&self, index: usize) -> Result<()> {
        self.exec(self.command(["stash", "drop", &format!("stash@{{{index}}}")]))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stash_list() {
        let id = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
        let out = format!(
            "stash@{{0}}\x1f{id}\x1fOn main: wip login\x1f1704067200\n\
             stash@{{1}}\x1f{id}\x1fWIP on dev: 1a2b3c4 subject\x1f1704067100\n"
        );
        let list = parse_stash_list(out.as_bytes()).expect("parse");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].refname(), "stash@{0}");
        assert_eq!(list[0].message, "On main: wip login");
        assert_eq!(list[1].index, 1);
        assert!(parse_stash_list(b"garbage").is_err());
    }
}
