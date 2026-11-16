//! Reflog and undo. Every HEAD movement is in the reflog, so "undo" means
//! moving the current branch back to where it was — with `reset --keep`,
//! which refuses instead of overwriting uncommitted work.

use gf_core::model::Oid;
use gf_core::{GfError, Result};

use crate::backend::{CliBackend, LogOps, ReflogOps};

/// How `git reset` treats the index and the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResetMode {
    /// Changes of the commits left behind stay staged.
    #[default]
    Soft,
    /// They stay in the working tree, unstaged.
    Mixed,
    /// Index and working tree match the target: uncommitted work is lost.
    Hard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    pub id: Oid,
    /// `HEAD@{2}`
    pub selector: String,
    /// e.g. `commit: fix bug`, `checkout: moving from a to b`.
    pub message: String,
}

pub fn parse_reflog(out: &[u8]) -> Result<Vec<ReflogEntry>> {
    String::from_utf8_lossy(out)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let mut f = line.splitn(3, '\x1f');
            let (Some(id), Some(selector), Some(message)) = (f.next(), f.next(), f.next()) else {
                return Err(GfError::parse("reflog", line.to_owned()));
            };
            Ok(ReflogEntry {
                id: id.parse()?,
                selector: selector.to_owned(),
                message: message.to_owned(),
            })
        })
        .collect()
}

impl ReflogOps for CliBackend {
    async fn reflog(&self, limit: usize) -> Result<Vec<ReflogEntry>> {
        let out = self
            .read(self.command([
                "reflog",
                "show",
                &format!("-n{limit}"),
                "--format=%H%x1f%gd%x1f%gs",
                "HEAD",
            ]))
            .await?;
        parse_reflog(&out)
    }

    async fn head_oid(&self) -> Result<Option<Oid>> {
        if !self.head_exists().await? {
            return Ok(None);
        }
        let out = self.read(self.command(["rev-parse", "HEAD"])).await?;
        String::from_utf8_lossy(&out).trim().parse().map(Some)
    }

    async fn reset(&self, to: &Oid, mode: ResetMode) -> Result<()> {
        let flag = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        self.exec(self.command(["reset", flag, "--end-of-options", to.as_str()]))
            .await?;
        Ok(())
    }

    async fn undo_to(&self, to: &Oid, soft: bool) -> Result<()> {
        let mode = if soft { "--soft" } else { "--keep" };
        self.exec(self.command(["reset", mode, "--end-of-options", to.as_str()]))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reflog() {
        let id = "a".repeat(40);
        let out = format!(
            "{id}\x1fHEAD@{{0}}\x1fcommit: two\n{id}\x1fHEAD@{{1}}\x1fcheckout: moving from a to b\n"
        );
        let r = parse_reflog(out.as_bytes()).expect("parse");
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].message, "checkout: moving from a to b");
        assert!(parse_reflog(b"bad").is_err());
    }
}
