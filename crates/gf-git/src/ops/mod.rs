//! High-level repository operations, grouped by area. Each file adds an
//! `impl Repository` block, so new areas (stash, rebase, submodule, ...) are
//! added as new files without touching existing ones.

pub mod bisect;
pub mod blame;
pub mod branch;
pub mod commit;
pub mod conflict;
pub mod diff;
pub mod index;
pub mod integrate;
pub mod lfs;
pub mod log;
pub mod rebase_interactive;
pub mod reflog;
pub mod refs;
pub mod remote;
pub mod stash;
pub mod state;
pub mod status;
pub mod submodule;
pub mod sync;
pub mod tag;
pub mod worktree;
