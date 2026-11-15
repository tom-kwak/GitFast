use super::{Oid, Timestamp};

/// One entry of `git stash list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// Position in the stash stack: `stash@{index}`.
    pub index: usize,
    pub id: Oid,
    /// e.g. `WIP on main: 1a2b3c4 subject` or `On main: my message`.
    pub message: String,
    pub time: Timestamp,
}

impl StashEntry {
    pub fn refname(&self) -> String {
        format!("stash@{{{}}}", self.index)
    }
}
