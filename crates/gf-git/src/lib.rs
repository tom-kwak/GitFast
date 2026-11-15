//! Git backend for gf.
//!
//! - [`cli`]: runs the system `git` executable. Every write and every git
//!   subcommand goes through [`GitRunner`], so hooks, credential helpers, LFS
//!   and signing behave exactly as in the user's terminal.
//! - [`cli::parse`]: parsers for git's machine-readable output formats.
//! - [`Repository`]: the per-repository facade the UI uses. Its read methods
//!   live in [`ops`], one file per area.
//! - `gix_reader` (feature `gix`): in-process fast paths for hot reads, with
//!   the CLI as fallback.

pub mod cli;
pub mod conflict;
#[cfg(feature = "gix")]
mod gix_reader;
pub mod graph;
pub mod ops;
pub mod patch;
mod repository;

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
pub use ops::rebase_interactive::{TodoAction, TodoItem, melds_into_nothing};
pub use ops::reflog::ReflogEntry;
pub use ops::stash::StashOptions;
pub use ops::state::RepoState;
pub use ops::submodule::{Submodule, SubmoduleStatus};
pub use ops::sync::{PullMode, PushOptions};
pub use ops::worktree::Worktree;
pub use repository::Repository;
pub use tokio_util::sync::CancellationToken;
