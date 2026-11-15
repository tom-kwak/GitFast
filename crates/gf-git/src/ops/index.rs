//! Staging area: stage, unstage, discard, and applying partial patches.

use std::path::PathBuf;

use gf_core::Result;

use crate::Repository;

impl Repository {
    /// Stages the given paths, including deletions (`git add -A -- paths`).
    pub async fn stage(&self, paths: &[PathBuf]) -> Result<()> {
        self.exec(self.command(["add", "-A"]).paths(paths)).await?;
        Ok(())
    }

    /// Stages every change in the working tree.
    pub async fn stage_all(&self) -> Result<()> {
        self.exec(self.command(["add", "-A"])).await?;
        Ok(())
    }

    /// Removes the given paths from the index, keeping working tree content.
    pub async fn unstage(&self, paths: &[PathBuf]) -> Result<()> {
        let cmd = if self.head_exists().await? {
            self.command(["restore", "--staged"]).paths(paths)
        } else {
            // Before the first commit there is no HEAD to restore from.
            self.command(["rm", "--cached", "-r", "--quiet"])
                .paths(paths)
        };
        self.exec(cmd).await?;
        Ok(())
    }

    pub async fn unstage_all(&self) -> Result<()> {
        if self.head_exists().await? {
            self.exec(self.command(["reset", "--quiet"])).await?;
        } else {
            self.exec(self.command(["rm", "--cached", "-r", "--quiet", "."]))
                .await?;
        }
        Ok(())
    }

    /// Throws away unstaged changes to tracked files and deletes the given
    /// untracked files. Irreversible: callers must confirm with the user.
    pub async fn discard(&self, tracked: &[PathBuf], untracked: &[PathBuf]) -> Result<()> {
        if !tracked.is_empty() {
            self.exec(self.command(["restore", "--worktree"]).paths(tracked))
                .await?;
        }
        if !untracked.is_empty() {
            self.exec(self.command(["clean", "-f", "-d"]).paths(untracked))
                .await?;
        }
        Ok(())
    }

    /// Applies a patch to the index (`cached`) or the working tree.
    /// `reverse` un-applies it. Used for hunk and line staging with patches
    /// from [`crate::patch`].
    pub async fn apply_patch(&self, patch: String, cached: bool, reverse: bool) -> Result<()> {
        let mut cmd = self.command(["apply", "--recount", "--whitespace=nowarn"]);
        if cached {
            cmd = cmd.arg("--cached");
        }
        if reverse {
            cmd = cmd.arg("--reverse");
        }
        self.exec(cmd.arg("-").stdin(patch)).await?;
        Ok(())
    }
}
