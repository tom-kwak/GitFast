//! Git LFS (needs the `git-lfs` extension installed).

use std::path::PathBuf;

use gf_core::Result;

use crate::backend::{CliBackend, LfsOps};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsFile {
    pub path: PathBuf,
    /// The content is present locally (`*`) rather than only a pointer (`-`).
    pub downloaded: bool,
}

/// Patterns from `git lfs track` output: `    *.psd (.gitattributes)`.
pub fn parse_tracked(out: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(out)
        .lines()
        .filter(|l| l.starts_with(' ') || l.starts_with('\t'))
        .filter_map(|l| {
            let l = l.trim();
            let pattern = l.rsplit_once(" (").map_or(l, |(p, _)| p);
            (!pattern.is_empty()).then(|| pattern.to_owned())
        })
        .collect()
}

/// `git lfs ls-files` lines: `<oid> <*|-> <path>`.
pub fn parse_ls_files(out: &[u8]) -> Vec<LfsFile> {
    String::from_utf8_lossy(out)
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, ' ');
            let (_oid, mark, path) = (parts.next()?, parts.next()?, parts.next()?);
            Some(LfsFile {
                path: PathBuf::from(path),
                downloaded: mark == "*",
            })
        })
        .collect()
}

impl LfsOps for CliBackend {
    async fn lfs_available(&self) -> bool {
        let cmd = self.command(["lfs", "version"]).read_only().allow_failure();
        self.runner().run(&cmd).await.is_ok_and(|o| o.success())
    }

    async fn lfs_tracked(&self) -> Result<Vec<String>> {
        Ok(parse_tracked(
            &self.read(self.command(["lfs", "track"])).await?,
        ))
    }

    async fn lfs_files(&self) -> Result<Vec<LfsFile>> {
        Ok(parse_ls_files(
            &self.read(self.command(["lfs", "ls-files"])).await?,
        ))
    }

    async fn lfs_track(&self, pattern: &str) -> Result<()> {
        self.exec(self.command(["lfs", "track", pattern])).await?;
        Ok(())
    }

    async fn lfs_untrack(&self, pattern: &str) -> Result<()> {
        self.exec(self.command(["lfs", "untrack", pattern])).await?;
        Ok(())
    }

    async fn lfs_pull(&self) -> Result<()> {
        self.exec(self.command(["lfs", "pull"])).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_track_and_ls_files() {
        let track = b"Listing tracked patterns\n    *.psd (.gitattributes)\n    assets/** (assets/.gitattributes)\nListing excluded patterns\n";
        assert_eq!(parse_tracked(track), ["*.psd", "assets/**"]);
        let files = parse_ls_files(b"abc123 * art/logo.psd\ndef456 - big file.bin\n");
        assert_eq!(files.len(), 2);
        assert!(files[0].downloaded && !files[1].downloaded);
        assert_eq!(files[1].path, PathBuf::from("big file.bin"));
    }
}
