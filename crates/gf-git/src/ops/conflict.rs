//! Resolving conflicted files: whole-file sides, or block-by-block choices
//! written back from [`crate::conflict`].

use std::io::Write;
use std::path::{Component, Path, PathBuf};

use gf_core::{GfError, Result};

use crate::backend::{CliBackend, ConflictOps, DiffOps, IndexOps};
use crate::cli::RunOptions;
use crate::conflict::{self, Choice, Segment};
use crate::ops::diff::{FILE_PREVIEW_LIMIT, FileVersion};

/// Whole-file side to take for a conflicted path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
}

/// Inputs of a conflict resolution session. The index bytes identify exactly
/// the stages that were present when the draft was opened.
#[derive(Debug, PartialEq, Eq)]
pub struct ConflictDocument {
    index: Vec<u8>,
    pub worktree: Option<String>,
    /// Base, ours and theirs, in index stage order. An absent entry can
    /// represent a deletion, or a file that cannot be shown as text.
    pub sources: [Option<String>; 3],
    pub present: [bool; 3],
    pub marker_width: usize,
}

impl ConflictDocument {
    /// True when the same set of unmerged index entries still exists.
    pub fn same_index(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl ConflictOps for CliBackend {
    async fn conflict_document(&self, path: &Path) -> Result<ConflictDocument> {
        self.conflict_path(path).await?;
        let (index, worktree, marker_width) = tokio::try_join!(
            self.unmerged(path),
            self.conflict_text(path),
            self.marker_width(path)
        )?;
        let stages = stage_blobs(&index)?;
        let (base, ours, theirs) = tokio::try_join!(
            self.conflict_blob(stages[0]),
            self.conflict_blob(stages[1]),
            self.conflict_blob(stages[2]),
        )?;
        let present = stages.map(|entry| entry.is_some());
        Ok(ConflictDocument {
            index,
            worktree,
            sources: [base, ours, theirs],
            present,
            marker_width,
        })
    }

    async fn save_conflict_result(
        &self,
        path: &Path,
        original: &ConflictDocument,
        result: &str,
    ) -> Result<()> {
        let full = self.conflict_path(path).await?;
        if result.len() > FILE_PREVIEW_LIMIT || conflict::has_markers(result, original.marker_width)
        {
            return Err(GfError::Other(
                "result is too large or still has conflict markers".into(),
            ));
        }
        let (index, current) = tokio::try_join!(self.unmerged(path), self.conflict_text(path))?;
        if index.is_empty()
            || index != original.index
            || current.is_none()
            || (current != original.worktree && current.as_deref() != Some(result))
        {
            return Err(GfError::Other(
                "conflict changed on disk; reopen it before saving".into(),
            ));
        }
        if current.as_deref() != Some(result) {
            let expected = current.unwrap_or_default().into_bytes();
            let bytes = result.as_bytes().to_vec();
            tokio::task::spawn_blocking(move || write_result(&full, &expected, &bytes))
                .await
                .map_err(|e| GfError::Other(e.to_string()))??;
        }
        // The first write may succeed even if the index is temporarily locked.
        // A retry with the same result and unchanged stages can stage it.
        if self.unmerged(path).await? != original.index
            || self.conflict_text(path).await?.as_deref() != Some(result)
        {
            return Err(GfError::Other(
                "file or index changed before staging".into(),
            ));
        }
        self.stage_conflict_path(path).await
    }

    async fn mark_conflict_resolved(&self, path: &Path) -> Result<()> {
        self.conflict_path(path).await?;
        let (index, text, width) = tokio::try_join!(
            self.unmerged(path),
            self.conflict_text(path),
            self.marker_width(path)
        )?;
        if index.is_empty()
            || text
                .as_deref()
                .is_some_and(|text| conflict::has_markers(text, width))
        {
            return Err(GfError::Other(
                "path is no longer conflicted or still contains markers".into(),
            ));
        }
        self.stage_conflict_path(path).await
    }

    async fn conflict_segments(&self, path: &Path) -> Result<Option<Vec<Segment>>> {
        let full = self.workdir_path(path)?;
        let text = tokio::fs::read(&full).await?;
        Ok(String::from_utf8(text)
            .ok()
            .and_then(|t| conflict::parse(&t)))
    }

    async fn resolve_blocks(
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

    async fn take_side(&self, path: &Path, side: Side) -> Result<()> {
        self.conflict_path(path).await?;
        let index = self.unmerged(path).await?;
        let entries = stage_blobs(&index)?;
        let (flag, slot) = match side {
            Side::Ours => ("--ours", 1),
            Side::Theirs => ("--theirs", 2),
        };
        match (entries[slot], entries[3 - slot]) {
            (Some(_), _) => {
                self.exec(
                    self.command(["--literal-pathspecs", "checkout", flag])
                        .paths([path]),
                )
                .await?;
                self.stage_conflict_path(path).await
            }
            (None, Some(_)) => {
                self.exec(
                    self.command(["--literal-pathspecs", "rm", "--force"])
                        .paths([path]),
                )
                .await?;
                Ok(())
            }
            (None, None) => Err(GfError::Other(
                "no conflicting version remains for this path".into(),
            )),
        }
    }
}

impl CliBackend {
    async fn unmerged(&self, path: &Path) -> Result<Vec<u8>> {
        self.read(
            self.command(["--literal-pathspecs", "ls-files", "--unmerged", "-z"])
                .paths([path]),
        )
        .await
    }

    async fn stage_conflict_path(&self, path: &Path) -> Result<()> {
        self.exec(
            self.command(["--literal-pathspecs", "add", "-A"])
                .paths([path]),
        )
        .await?;
        Ok(())
    }

    async fn conflict_path(&self, path: &Path) -> Result<PathBuf> {
        if path.as_os_str().is_empty()
            || !path.components().all(|p| matches!(p, Component::Normal(_)))
        {
            return Err(GfError::Other(
                "expected a repository-relative file path".into(),
            ));
        }
        let full = self.workdir_path(path)?;
        let root = tokio::fs::canonicalize(self.cwd()).await?;
        let relative_parent = path.parent().unwrap_or_else(|| Path::new(""));
        let mut parent = root;
        for part in relative_parent.components() {
            parent.push(part);
            match tokio::fs::symlink_metadata(&parent).await {
                Ok(meta) if !meta.is_dir() => {
                    return Err(GfError::Unsupported(
                        "conflict path passes through a link or file",
                    ));
                }
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                Ok(_) | Err(_) => {}
            }
        }
        Ok(full)
    }

    async fn marker_width(&self, path: &Path) -> Result<usize> {
        let output = self
            .read(
                self.command(["check-attr", "-z", "conflict-marker-size"])
                    .paths([path]),
            )
            .await?;
        let value = output.split(|b| *b == 0).nth(2).unwrap_or_default();
        Ok(std::str::from_utf8(value)
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|width| *width > 0)
            .unwrap_or(7))
    }

    async fn conflict_text(&self, path: &Path) -> Result<Option<String>> {
        let full = self.workdir_path(path)?;
        match tokio::fs::symlink_metadata(full).await {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        match DiffOps::file_bytes(self, path, &FileVersion::WorkTree).await {
            Ok(bytes) => Ok(decode_text(bytes)),
            Err(GfError::FilePreviewTooLarge) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn conflict_blob(&self, entry: Option<(&str, &str)>) -> Result<Option<String>> {
        let Some(("100644" | "100755", oid)) = entry else {
            return Ok(None);
        };
        let cmd = self
            .command(["--no-replace-objects", "cat-file", "blob", "--", oid])
            .read_only();
        let opts = RunOptions {
            max_stdout: Some(FILE_PREVIEW_LIMIT),
            ..RunOptions::default()
        };
        match self.runner().run_with(&cmd, opts).await {
            Ok(output) => Ok(decode_text(output.stdout)),
            Err(GfError::OutputTooLarge { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn workdir_path(&self, path: &Path) -> Result<PathBuf> {
        let root = self.handle().workdir.as_ref().ok_or(GfError::Unsupported(
            "bare repositories have no working tree",
        ))?;
        Ok(root.join(path))
    }
}

fn decode_text(bytes: Vec<u8>) -> Option<String> {
    (!bytes.contains(&0))
        .then(|| String::from_utf8(bytes).ok())
        .flatten()
}

fn stage_blobs(index: &[u8]) -> Result<[Option<(&str, &str)>; 3]> {
    let mut stages = [None; 3];
    for record in index.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let header = record.split(|b| *b == b'\t').next().unwrap_or_default();
        let header = std::str::from_utf8(header)
            .map_err(|e| GfError::parse("unmerged index", e.to_string()))?;
        let mut fields = header.split_ascii_whitespace();
        if let (Some(mode), Some(id), Some(stage)) = (fields.next(), fields.next(), fields.next()) {
            let slot = match stage {
                "1" => 0,
                "2" => 1,
                "3" => 2,
                _ => return Err(GfError::parse("unmerged index", header)),
            };
            stages[slot] = Some((mode, id));
        } else {
            return Err(GfError::parse("unmerged index", header));
        }
    }
    Ok(stages)
}

fn write_result(full: &Path, expected: &[u8], result: &[u8]) -> Result<()> {
    let meta = std::fs::symlink_metadata(full)?;
    let parent = full
        .parent()
        .ok_or(GfError::Unsupported("file has no parent"))?;
    if !meta.is_file() || meta.len() > FILE_PREVIEW_LIMIT as u64 {
        return Err(GfError::Other(
            "conflict changed on disk; reopen it before saving".into(),
        ));
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(result)?;
    temp.as_file().set_permissions(meta.permissions())?;
    let mut current = Vec::new();
    use std::io::Read;
    std::fs::File::open(full)?
        .take(FILE_PREVIEW_LIMIT as u64 + 1)
        .read_to_end(&mut current)?;
    if current != expected {
        return Err(GfError::Other(
            "conflict changed on disk; reopen it before saving".into(),
        ));
    }
    temp.persist(full).map_err(|e| e.error)?;
    Ok(())
}
