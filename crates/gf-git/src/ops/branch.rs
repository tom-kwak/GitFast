//! Branch management.

use gf_core::{GfError, Result};

use crate::backend::{BranchOps, CliBackend};

impl BranchOps for CliBackend {
    async fn check_branch_name(&self, name: &str) -> Result<Option<String>> {
        if name.trim().is_empty() {
            return Ok(Some("name is empty".into()));
        }
        let cmd = self
            .command(["check-ref-format", "--branch", name])
            .read_only()
            .allow_failure();
        let out = self.runner().run(&cmd).await?;
        Ok((!out.success()).then(|| format!("`{name}` is not a valid branch name")))
    }

    async fn create_branch(&self, name: &str, start: Option<&str>, checkout: bool) -> Result<()> {
        let mut cmd = if checkout {
            self.command(["switch", "-c", name])
        } else {
            self.command(["branch", name])
        };
        if let Some(start) = start {
            cmd = cmd.arg(start);
        }
        self.exec(cmd).await?;
        Ok(())
    }

    async fn switch_branch(&self, name: &str, remote: bool) -> Result<()> {
        let cmd = if remote {
            let local = name
                .split_once('/')
                .map(|(_, rest)| rest)
                .ok_or_else(|| GfError::Other(format!("not a remote branch: {name}")))?;
            self.command(["switch", "--track", "-c", local, name])
        } else {
            self.command(["switch", name])
        };
        self.exec(cmd).await?;
        Ok(())
    }

    async fn checkout_detached(&self, rev: &str) -> Result<()> {
        self.exec(self.command(["switch", "--detach", rev])).await?;
        Ok(())
    }

    async fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        let flag = if force { "-D" } else { "-d" };
        self.exec(self.command(["branch", flag, name])).await?;
        Ok(())
    }

    async fn rename_branch(&self, old: &str, new: &str) -> Result<()> {
        self.exec(self.command(["branch", "-m", old, new])).await?;
        Ok(())
    }

    async fn delete_remote_branch(&self, remote: &str, branch: &str) -> Result<()> {
        self.exec(self.command(["push", remote, "--delete", branch]))
            .await?;
        Ok(())
    }
}
