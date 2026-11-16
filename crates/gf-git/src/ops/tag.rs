//! Tag management.

use gf_core::Result;

use crate::backend::{CliBackend, TagOps};

impl TagOps for CliBackend {
    async fn create_tag(
        &self,
        name: &str,
        target: Option<&str>,
        message: Option<&str>,
    ) -> Result<()> {
        let mut cmd = match message {
            Some(_) => self.command(["tag", "-a", name, "-F", "-"]),
            None => self.command(["tag", name]),
        };
        if let Some(target) = target {
            cmd = cmd.arg(target);
        }
        if let Some(message) = message {
            cmd = cmd.stdin(message.as_bytes().to_vec());
        }
        self.exec(cmd).await?;
        Ok(())
    }

    async fn delete_tag(&self, name: &str) -> Result<()> {
        self.exec(self.command(["tag", "-d", name])).await?;
        Ok(())
    }

    async fn push_tag(&self, remote: &str, name: &str) -> Result<()> {
        self.exec(self.command(["push", remote, &format!("refs/tags/{name}")]))
            .await?;
        Ok(())
    }
}
