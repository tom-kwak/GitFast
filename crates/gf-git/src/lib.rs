//! Git backend for gf.
//!
//! - [`cli`]: runs the system `git` executable. Every write and every git
//!   subcommand goes through [`GitRunner`], so hooks, credential helpers, LFS
//!   and signing behave exactly as in the user's terminal.
//! - [`cli::parse`]: parsers for git's machine-readable output formats.
//! - [`backend`]: the git interface, one trait per area, and
//!   [`backend::CliBackend`], which implements all of them with the git CLI
//!   (implementations in [`ops`], next to each area's types).
//! - [`Repository`]: the per-repository facade the UI uses. `routes.rs`
//!   decides which backend serves each command (`docs/GIX_MIGRATION.md`).
//! - `backend::GixBackend` (feature `gix`): in-process reads through
//!   gitoxide, with the CLI as fallback (`GF_GIT_BACKEND=cli` turns it off).

pub mod backend;
pub mod cli;
pub mod conflict;
pub mod graph;
pub mod ops;
pub mod patch;
mod repository;
mod routes;

pub use cli::parse::numstat::LineCounts;
pub use cli::{GitCommand, GitOutput, GitRunner, Progress, RunOptions};
pub use ops::bisect::{BisectProgress, Verdict};
pub use ops::blame::BlameLine;
pub use ops::commit::CommitOptions;
pub use ops::conflict::Side;
pub use ops::diff::{DiffOptions, DiffTarget};
pub use ops::integrate::{Integration, MergeStyle};
pub use ops::lfs::LfsFile;
pub use ops::log::{LogOrder, LogQuery};
pub use ops::rebase_interactive::{
    SquashPreview, TodoAction, TodoItem, melds_into_nothing, squash_plan, squash_range,
};
pub use ops::reflog::{ReflogEntry, ResetMode};
pub use ops::stash::StashOptions;
pub use ops::state::RepoState;
pub use ops::submodule::{Submodule, SubmoduleStatus};
pub use ops::sync::{PullMode, PushOptions};
pub use ops::undo::{Capture, HeadMode, Scope};
pub use ops::worktree::Worktree;
pub use repository::Repository;
pub use tokio_util::sync::CancellationToken;
