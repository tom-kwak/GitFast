use gf_core::Result;
use gf_core::model::{Branch, Oid, Tag};

use crate::backend::{CliBackend, LogOps, RefOps};
use crate::cli::parse::refs::{BRANCH_ARGS, TAG_ARGS, parse_branches, parse_tags};

impl RefOps for CliBackend {
    async fn branches(&self) -> Result<Vec<Branch>> {
        parse_branches(&self.read(self.command(BRANCH_ARGS)).await?)
    }

    async fn tags(&self) -> Result<Vec<Tag>> {
        parse_tags(&self.read(self.command(TAG_ARGS)).await?)
    }

    async fn unpushed_count(&self) -> Result<u32> {
        if !self.head_exists().await? {
            return Ok(0);
        }
        let out = self
            .read(self.command(["rev-list", "--count", "HEAD", "--not", "--remotes"]))
            .await?;
        String::from_utf8_lossy(&out).trim().parse().map_err(|_| {
            gf_core::GfError::parse(
                "rev-list --count",
                String::from_utf8_lossy(&out).into_owned(),
            )
        })
    }

    async fn unpushed(&self, since: Option<&Oid>) -> Result<Vec<Oid>> {
        if !self.head_exists().await? {
            return Ok(Vec::new());
        }
        let mut cmd = self.command(["rev-list", "HEAD", "--not", "--remotes"]);
        if let Some(b) = since {
            cmd = cmd.arg(format!("^{b}"));
        }
        String::from_utf8_lossy(&self.read(cmd).await?)
            .lines()
            .map(|l| l.trim().parse())
            .collect()
    }
}
