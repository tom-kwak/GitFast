use gf_core::Result;
use gf_core::model::Remote;

use crate::Repository;
use crate::cli::parse::remote::{REMOTE_ARGS, parse_remotes};

impl Repository {
    pub async fn remotes(&self) -> Result<Vec<Remote>> {
        parse_remotes(&self.read(self.command(REMOTE_ARGS)).await?)
    }
}
