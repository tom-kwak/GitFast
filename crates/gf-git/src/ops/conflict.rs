//! Resolving conflicted files: whole-file sides, or block-by-block choices
//! written back from [`crate::conflict`].

use std::path::{Path, PathBuf};

use gf_core::{GfError, Result};

use crate::Repository;
use crate::conflict::{self, Choice, Segment};

/// Whole-file side to take for a conflicted path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
}

impl Repository {
    fn workdir_path(&self, path: &Path) -> Result<PathBuf> {
        let root = self.handle().workdir.as_ref().ok_or(GfError::Unsupported(
            "bare repositories have no working tree",
        ))?;
        Ok(root.join(path))
    }

    /// The conflicted file split into common text and conflict blocks.
    /// `None` when the file has no parseable markers (binary, or edited).
    pub async fn conflict_segments(&self, path: &Path) -> Result<Option<Vec<Segment>>> {
        let full = self.workdir_path(path)?;
        let text = tokio::fs::read(&full).await?;
        Ok(String::from_utf8(text)
            .ok()
            .and_then(|t| conflict::parse(&t)))
    }

    /// Writes the file with `choices` applied (unchosen blocks keep their
    /// markers) and stages it when no conflict is left.
    /// Returns how many conflicts remain.
    pub async fn resolve_blocks(
        &self,
        path: &Path,
        segments: &[Segment],
        choices: &[Option<Choice>],
    ) -> Result<usize> {
        let full = self.workdir_path(path)?;
        let text = conflict::render(segments, choices);
        tokio::fs::write(&full, &text).await?;
        let left = conflict::parse(&text).map_or(0, |s| conflict::conflict_count(&s));
        if left == 0 {
            self.stage(&[path.to_path_buf()]).await?;
        }
        Ok(left)
    }

    /// Resolves the whole file with one side and stages it.
    pub async fn take_side(&self, path: &Path, side: Side) -> Result<()> {
        let flag = match side {
            Side::Ours => "--ours",
            Side::Theirs => "--theirs",
        };
        self.exec(self.command(["checkout", flag]).paths([path]))
            .await?;
        self.stage(&[path.to_path_buf()]).await
    }
}
