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
