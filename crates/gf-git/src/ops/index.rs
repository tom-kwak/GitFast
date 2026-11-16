//! Staging area: stage, unstage, discard, and applying partial patches.

use std::path::PathBuf;

use gf_core::Result;

use crate::backend::{CliBackend, IndexOps, LogOps};

/// Paths of submodules (gitlinks, mode 160000) in `git ls-files -s -z`
/// output: `<mode> <oid> <stage>\t<path>\0`.
fn parse_gitlinks(out: &[u8]) -> Vec<PathBuf> {
    out.split(|b| *b == 0)
        .filter_map(|rec| {
            let rec = std::str::from_utf8(rec).ok()?;
            let (meta, path) = rec.split_once('\t')?;
            meta.starts_with("160000 ").then(|| PathBuf::from(path))
        })
        .collect()
}

impl IndexOps for CliBackend {
    async fn stage(&self, paths: &[PathBuf]) -> Result<()> {
        self.exec(self.command(["add", "-A"]).paths(paths)).await?;
        Ok(())
    }

    async fn stage_all(&self) -> Result<()> {
        self.exec(self.command(["add", "-A"])).await?;
        Ok(())
    }

    async fn unstage(&self, paths: &[PathBuf]) -> Result<()> {
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

    async fn unstage_all(&self) -> Result<()> {
        if self.head_exists().await? {
            self.exec(self.command(["reset", "--quiet"])).await?;
        } else {
            self.exec(self.command(["rm", "--cached", "-r", "--quiet", "."]))
                .await?;
        }
        Ok(())
    }

    async fn discard(&self, tracked: &[PathBuf], untracked: &[PathBuf]) -> Result<()> {
        // `git restore` (and reverse-applying a `Subproject commit` patch)
        // succeeds on a submodule but leaves it as it is: a submodule is
        // put back by checking out the commit the superproject records,
        // in it and in the submodules nested inside it.
        let gitlinks = if tracked.is_empty() {
            Vec::new()
        } else {
            parse_gitlinks(
                &self
                    .read(self.command(["ls-files", "-s", "-z"]).paths(tracked))
                    .await?,
            )
        };
        let files: Vec<&PathBuf> = tracked.iter().filter(|p| !gitlinks.contains(p)).collect();
        if !files.is_empty() {
            self.exec(self.command(["restore", "--worktree"]).paths(files))
                .await?;
        }
        if !gitlinks.is_empty() {
            self.exec(
                self.command(["submodule", "update", "--init", "--recursive", "--force"])
                    .paths(&gitlinks),
            )
            .await?;
        }
        if !untracked.is_empty() {
            self.exec(self.command(["clean", "-f", "-d"]).paths(untracked))
                .await?;
        }
        Ok(())
    }

    async fn apply_patch(&self, patch: String, cached: bool, reverse: bool) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitlinks_are_found_by_mode() {
        let out = b"100644 0123456789abcdef0123456789abcdef01234567 0\tsrc/a b.rs\0\
160000 89abcdef0123456789abcdef0123456789abcdef 0\tvendor/lib\0";
        assert_eq!(parse_gitlinks(out), [PathBuf::from("vendor/lib")]);
        assert!(parse_gitlinks(b"").is_empty());
    }
}
