use std::path::PathBuf;

/// How a file changed between the two sides of a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileChange {
    Added,
    Deleted,
    Modified,
    /// `similarity` is git's 0–100 similarity index.
    Renamed {
        similarity: u8,
    },
    Copied {
        similarity: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line text without the leading `+`/`-`/` ` marker and without the
    /// trailing newline.
    pub content: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    /// Followed by `\ No newline at end of file`.
    pub no_newline_at_eof: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// Function/section context git prints after the second `@@`.
    pub section: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// `None` when the file was added.
    pub old_path: Option<PathBuf>,
    /// `None` when the file was deleted.
    pub new_path: Option<PathBuf>,
    pub change: FileChange,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    pub is_binary: bool,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// The path to show for this entry (new path, or old path if deleted).
    pub fn display_path(&self) -> Option<&PathBuf> {
        self.new_path.as_ref().or(self.old_path.as_ref())
    }

    pub fn mode_changed(&self) -> bool {
        matches!((&self.old_mode, &self.new_mode), (Some(a), Some(b)) if a != b)
    }

    /// `(added, removed)` line counts.
    pub fn line_stats(&self) -> (usize, usize) {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .fold((0, 0), |(a, r), l| match l.kind {
                LineKind::Added => (a + 1, r),
                LineKind::Removed => (a, r + 1),
                LineKind::Context => (a, r),
            })
    }
}
