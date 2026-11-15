use std::fmt;
use std::path::PathBuf;

use uuid::Uuid;

/// Identifies an open repository for the lifetime of the app session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RepoId(Uuid);

impl RepoId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RepoId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// An opened repository's locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoHandle {
    pub id: RepoId,
    /// Working tree root; `None` for bare repositories.
    pub workdir: Option<PathBuf>,
    /// The `.git` directory (or the repository itself when bare).
    pub git_dir: PathBuf,
}

impl RepoHandle {
    pub fn is_bare(&self) -> bool {
        self.workdir.is_none()
    }

    /// Display name: the working tree folder name, or the git dir name.
    pub fn display_name(&self) -> String {
        self.workdir
            .as_ref()
            .unwrap_or(&self.git_dir)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}
