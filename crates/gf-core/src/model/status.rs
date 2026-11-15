use std::path::PathBuf;

use super::Oid;

/// HEAD state reported alongside status (`# branch.*` headers).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeadInfo {
    /// `None` before the first commit.
    pub oid: Option<Oid>,
    /// Short branch name; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Upstream short name, e.g. `origin/main`.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusReport {
    pub head: HeadInfo,
    pub entries: Vec<StatusEntry>,
}

/// One side (index or working tree) of a status entry, matching the `X`/`Y`
/// letters of `git status --porcelain=v2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Unmodified,
    Modified,
    TypeChanged,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
}

impl ChangeKind {
    /// Parses a porcelain status letter (`.` means unmodified).
    pub fn from_porcelain(c: char) -> Option<Self> {
        Some(match c {
            '.' | ' ' => Self::Unmodified,
            'M' => Self::Modified,
            'T' => Self::TypeChanged,
            'A' => Self::Added,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            'U' => Self::Unmerged,
            _ => return None,
        })
    }
}

/// Which sides of a merge conflict touched the path (porcelain `XY` for
/// unmerged entries, e.g. `UU` = both modified).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictKind {
    BothDeleted,
    AddedByUs,
    DeletedByThem,
    AddedByThem,
    DeletedByUs,
    BothAdded,
    BothModified,
}

impl ConflictKind {
    pub fn from_porcelain(xy: &str) -> Option<Self> {
        Some(match xy {
            "DD" => Self::BothDeleted,
            "AU" => Self::AddedByUs,
            "UD" => Self::DeletedByThem,
            "UA" => Self::AddedByThem,
            "DU" => Self::DeletedByUs,
            "AA" => Self::BothAdded,
            "UU" => Self::BothModified,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusEntry {
    /// Tracked path with staged and/or unstaged changes.
    Changed {
        path: PathBuf,
        /// Source path for renames/copies.
        orig_path: Option<PathBuf>,
        index: ChangeKind,
        worktree: ChangeKind,
    },
    Conflicted {
        path: PathBuf,
        kind: ConflictKind,
    },
    Untracked {
        path: PathBuf,
    },
    Ignored {
        path: PathBuf,
    },
}

impl StatusEntry {
    pub fn path(&self) -> &PathBuf {
        match self {
            Self::Changed { path, .. }
            | Self::Conflicted { path, .. }
            | Self::Untracked { path }
            | Self::Ignored { path } => path,
        }
    }

    pub fn is_staged(&self) -> bool {
        matches!(self, Self::Changed { index, .. } if *index != ChangeKind::Unmodified)
    }

    pub fn has_unstaged_changes(&self) -> bool {
        match self {
            Self::Changed { worktree, .. } => *worktree != ChangeKind::Unmodified,
            Self::Untracked { .. } | Self::Conflicted { .. } => true,
            Self::Ignored { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_letters() {
        assert_eq!(
            ChangeKind::from_porcelain('.'),
            Some(ChangeKind::Unmodified)
        );
        assert_eq!(ChangeKind::from_porcelain('R'), Some(ChangeKind::Renamed));
        assert_eq!(ChangeKind::from_porcelain('?'), None);
        assert_eq!(
            ConflictKind::from_porcelain("UU"),
            Some(ConflictKind::BothModified)
        );
        assert_eq!(ConflictKind::from_porcelain("MM"), None);
    }

    #[test]
    fn staged_and_unstaged_flags() {
        let partly_staged = StatusEntry::Changed {
            path: "a.rs".into(),
            orig_path: None,
            index: ChangeKind::Modified,
            worktree: ChangeKind::Modified,
        };
        assert!(partly_staged.is_staged());
        assert!(partly_staged.has_unstaged_changes());

        let untracked = StatusEntry::Untracked {
            path: "b.rs".into(),
        };
        assert!(!untracked.is_staged());
        assert!(untracked.has_unstaged_changes());
    }
}
