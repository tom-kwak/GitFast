use gf_core::Result;
use gf_core::model::StatusReport;

use crate::Repository;
use crate::cli::parse::status::{STATUS_ARGS, parse_status};

impl Repository {
    /// Working tree and index status plus HEAD/upstream info.
    pub async fn status(&self) -> Result<StatusReport> {
        let out = self.read(self.command(STATUS_ARGS)).await?;
        parse_status(&out)
    }
}
