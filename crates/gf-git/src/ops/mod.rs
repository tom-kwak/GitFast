//! Git operations, grouped by area: each file holds the area's types and
//! parsers and the git CLI implementation of its interface
//! (`impl XxxOps for CliBackend`, see [`crate::backend`]).

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
pub mod undo;
pub mod worktree;
