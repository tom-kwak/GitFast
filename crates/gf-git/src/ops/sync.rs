//! Talking to remotes: fetch, pull, push. All accept [`RunOptions`] so the
//! UI can show progress and offer cancel.

use gf_core::Result;

use crate::Repository;
use crate::cli::RunOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PullMode {
    /// Use the repository's `pull.rebase`/`pull.ff` configuration.
    #[default]
    Config,
    /// Only fast-forward; fail instead of creating a merge.
    FastForwardOnly,
    Merge,
    Rebase,
}

#[derive(Debug, Clone, Default)]
pub struct PushOptions {
    /// Remote name; `None` uses the branch's configured remote.
    pub remote: Option<String>,
    /// Branch (or refspec) to push; `None` pushes the current branch.
    pub branch: Option<String>,
    /// `-u`: remember the remote branch as upstream.
    pub set_upstream: bool,
    /// `--force-with-lease`: overwrite the remote branch only if it is
    /// still where we last saw it. Plain `--force` is never used.
    pub force_with_lease: bool,
    pub tags: bool,
}

impl Repository {
    /// `git fetch --prune` from one remote or all of them.
    pub async fn fetch(&self, remote: Option<&str>, opts: RunOptions) -> Result<()> {
        let mut cmd = self.command(["fetch", "--prune", "--progress"]);
        cmd = match remote {
            Some(r) => cmd.arg(r),
            None => cmd.arg("--all"),
        };
        self.exec_with(cmd, opts).await?;
        Ok(())
    }

    pub async fn pull(&self, mode: PullMode, opts: RunOptions) -> Result<()> {
        let mut cmd = self.command(["pull", "--progress"]);
        cmd = match mode {
            PullMode::Config => cmd,
            PullMode::FastForwardOnly => cmd.arg("--ff-only"),
            PullMode::Merge => cmd.arg("--no-rebase"),
            PullMode::Rebase => cmd.arg("--rebase"),
        };
        self.exec_with(cmd, opts).await?;
        Ok(())
    }

    pub async fn push(&self, push: &PushOptions, opts: RunOptions) -> Result<()> {
        let mut cmd = self.command(["push", "--progress"]);
        if push.set_upstream {
            cmd = cmd.arg("--set-upstream");
        }
        if push.force_with_lease {
            cmd = cmd.arg("--force-with-lease");
        }
        if push.tags {
            cmd = cmd.arg("--tags");
        }
        if let Some(remote) = &push.remote {
            cmd = cmd.arg(remote);
            if let Some(branch) = &push.branch {
                cmd = cmd.arg(branch);
            }
        }
        self.exec_with(cmd, opts).await?;
        Ok(())
    }
}
