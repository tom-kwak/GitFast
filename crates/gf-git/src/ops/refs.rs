use gf_core::Result;
use gf_core::model::{Branch, Tag};

use crate::Repository;
use crate::cli::parse::refs::{BRANCH_ARGS, TAG_ARGS, parse_branches, parse_tags};

impl Repository {
    /// Local and remote-tracking branches, with upstream ahead/behind counts.
    pub async fn branches(&self) -> Result<Vec<Branch>> {
        parse_branches(&self.read(self.command(BRANCH_ARGS)).await?)
    }

    pub async fn tags(&self) -> Result<Vec<Tag>> {
        parse_tags(&self.read(self.command(TAG_ARGS)).await?)
    }
}

impl Repository {
    /// Commits reachable from HEAD that no remote-tracking branch contains:
    /// what a first push of an unpublished branch would upload. `0` before
    /// the first commit.
    pub async fn unpushed_count(&self) -> Result<u32> {
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
}
