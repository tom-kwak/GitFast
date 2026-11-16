//! Linked working trees (`git worktree`).

use std::path::{Path, PathBuf};

use gf_core::model::Oid;
use gf_core::{GfError, Result};

use crate::backend::{CliBackend, WorktreeOps};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: Option<Oid>,
    /// Short branch name; `None` when detached.
    pub branch: Option<String>,
    pub bare: bool,
    pub locked: bool,
    /// Its directory is gone; `prune` cleans it up.
    pub prunable: bool,
}

/// Parses `git worktree list --porcelain` (blank-line separated records).
pub fn parse_worktrees(out: &[u8]) -> Result<Vec<Worktree>> {
    let text = String::from_utf8_lossy(out);
    let mut list = Vec::new();
    for record in text.split("\n\n").filter(|r| !r.trim().is_empty()) {
        let mut wt = Worktree {
            path: PathBuf::new(),
            head: None,
            branch: None,
            bare: false,
            locked: false,
            prunable: false,
        };
        for line in record.lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "worktree" => wt.path = PathBuf::from(value),
                "HEAD" => wt.head = value.parse().ok(),
                "branch" => wt.branch = Some(value.trim_start_matches("refs/heads/").to_owned()),
                "bare" => wt.bare = true,
                "locked" => wt.locked = true,
                "prunable" => wt.prunable = true,
                _ => {}
            }
        }
        if wt.path.as_os_str().is_empty() {
            return Err(GfError::parse("worktree list", record.to_owned()));
        }
        list.push(wt);
    }
    Ok(list)
}

impl WorktreeOps for CliBackend {
    async fn worktrees(&self) -> Result<Vec<Worktree>> {
        parse_worktrees(
            &self
                .read(self.command(["worktree", "list", "--porcelain"]))
                .await?,
        )
    }

    async fn add_worktree(&self, path: &Path, branch: &str, new_branch: bool) -> Result<()> {
        let mut cmd = self.command(["worktree", "add"]);
        if new_branch {
            cmd = cmd.args(["-b", branch]).arg(path);
        } else {
            cmd = cmd.arg(path).arg(branch);
        }
        self.exec(cmd).await?;
        Ok(())
    }

    async fn remove_worktree(&self, path: &Path, force: bool) -> Result<()> {
        let mut cmd = self.command(["worktree", "remove"]);
        if force {
            cmd = cmd.arg("--force");
        }
        self.exec(cmd.arg(path)).await?;
        Ok(())
    }

    async fn prune_worktrees(&self) -> Result<()> {
        self.exec(self.command(["worktree", "prune"])).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_records() {
        let id = "a".repeat(40);
        let out = format!(
            "worktree /repo\nHEAD {id}\nbranch refs/heads/main\n\n\
             worktree /repo-wt\nHEAD {id}\ndetached\nlocked reason\n\n\
             worktree /gone\nHEAD {id}\nbranch refs/heads/old\nprunable gitdir file points to non-existent location\n"
        );
        let w = parse_worktrees(out.as_bytes()).expect("parse");
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].branch.as_deref(), Some("main"));
        assert!(w[1].branch.is_none() && w[1].locked);
        assert!(w[2].prunable);
    }
}
