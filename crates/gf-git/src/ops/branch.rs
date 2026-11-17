//! Branch management, and what switching branch does with changes.

use std::path::PathBuf;

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

// ------------------------------------------------- switching with changes

impl CliBackend {
    /// Paths that differ between `HEAD` and `target`.
    pub(crate) async fn paths_differing_from_head(&self, target: &str) -> Result<Vec<PathBuf>> {
        let out = self
            .read(self.command([
                "diff",
                "--name-only",
                "-z",
                "--no-renames",
                "HEAD",
                target,
                "--",
            ]))
            .await?;
        Ok(out
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| PathBuf::from(String::from_utf8_lossy(p).into_owned()))
            .collect())
    }
}

/// The changed paths `git switch` would refuse over: a changed file (or an
/// untracked one, or a folder of them) is in the way when `target` has a
/// different version of it. git does not merge on switch, so this is
/// exactly its rule, not a guess.
pub fn switch_blockers(changed: &[PathBuf], differing: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = differing
        .iter()
        .filter(|d| changed.iter().any(|c| d.starts_with(c)))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Message of the stash gf makes when changes are left on `branch`.
pub fn left_on(branch: &str) -> String {
    format!("{LEFT_ON}{branch}")
}

/// The branch a stash left by gf belongs to (from its message, which git
/// shows as `On main: gf: left on main`).
pub fn left_on_branch(message: &str) -> Option<&str> {
    message
        .find(LEFT_ON)
        .map(|i| &message[i + LEFT_ON.len()..])
        .filter(|b| !b.is_empty())
}

const LEFT_ON: &str = "gf: left on ";

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn blockers_are_changed_paths_the_target_has_differently() {
        let changed = [p("a.rs"), p("notes.txt"), p("build/")];
        let differing = [p("a.rs"), p("b.rs"), p("build/x.o"), p("buildx")];
        assert_eq!(
            switch_blockers(&changed, &differing),
            [p("a.rs"), p("build/x.o")]
        );
        assert!(switch_blockers(&[], &differing).is_empty());
    }

    #[test]
    fn left_stashes_name_their_branch() {
        assert_eq!(left_on("feat/x"), "gf: left on feat/x");
        assert_eq!(left_on_branch("On main: gf: left on main"), Some("main"));
        assert_eq!(left_on_branch("On main: wip"), None);
        assert_eq!(left_on_branch("gf: left on "), None);
    }
}
