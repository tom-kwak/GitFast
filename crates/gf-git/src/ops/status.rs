use gf_core::Result;
use gf_core::model::StatusReport;

use crate::backend::{CliBackend, StatusOps};
use crate::cli::parse::status::{STATUS_ARGS, parse_status};

impl StatusOps for CliBackend {
    async fn status(&self) -> Result<StatusReport> {
        let out = self.read(self.command(STATUS_ARGS)).await?;
        parse_status(&out)
    }
}
