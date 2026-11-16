//! Submodules (`git submodule`).

use std::path::PathBuf;

use gf_core::model::Oid;
use gf_core::{GfError, Result};

use crate::backend::{CliBackend, SubmoduleOps};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmoduleStatus {
    /// Checked out at the commit the superproject records.
    UpToDate,
    /// Not initialized (`-`).
    Uninitialized,
    /// Checked out at a different commit (`+`).
    Modified,
    /// Merge conflict (`U`).
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    pub path: PathBuf,
    pub commit: Oid,
    pub status: SubmoduleStatus,
    /// `git describe` of the checked-out commit, if any.
    pub describe: Option<String>,
}

/// Parses `git submodule status --recursive` lines:
/// `<flag><oid> <path>[ (<describe>)]`.
pub fn parse_submodules(out: &[u8]) -> Result<Vec<Submodule>> {
    String::from_utf8_lossy(out)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let bad = || GfError::parse("submodule status", line.to_owned());
            let mut chars = line.chars();
            let status = match chars.next().ok_or_else(bad)? {
                ' ' => SubmoduleStatus::UpToDate,
                '-' => SubmoduleStatus::Uninitialized,
                '+' => SubmoduleStatus::Modified,
                'U' => SubmoduleStatus::Conflict,
                _ => return Err(bad()),
            };
            let rest = chars.as_str();
            let (oid, rest) = rest.split_once(' ').ok_or_else(bad)?;
            let (path, describe) = match rest.rsplit_once(" (") {
                Some((p, d)) => (p, d.strip_suffix(')').map(str::to_owned)),
                None => (rest, None),
            };
            Ok(Submodule {
                path: PathBuf::from(path),
                commit: oid.parse()?,
                status,
                describe,
            })
        })
        .collect()
}

impl SubmoduleOps for CliBackend {
    async fn submodules(&self) -> Result<Vec<Submodule>> {
        parse_submodules(
            &self
                .read(self.command(["submodule", "status", "--recursive"]))
                .await?,
        )
    }

    async fn update_submodules(&self) -> Result<()> {
        self.exec(self.command(["submodule", "update", "--init", "--recursive"]))
            .await?;
        Ok(())
    }

    async fn sync_submodules(&self) -> Result<()> {
        self.exec(self.command(["submodule", "sync", "--recursive"]))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_lines() {
        let id = "a".repeat(40);
        let out = format!(
            " {id} libs/core (v1.2-3-gabc)\n-{id} vendor/x\n+{id} with space (heads/main)\n"
        );
        let s = parse_submodules(out.as_bytes()).expect("parse");
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].status, SubmoduleStatus::UpToDate);
        assert_eq!(s[0].describe.as_deref(), Some("v1.2-3-gabc"));
        assert_eq!(s[1].status, SubmoduleStatus::Uninitialized);
        assert_eq!(s[2].path, PathBuf::from("with space"));
        assert!(parse_submodules(b"?bad").is_err());
    }
}
