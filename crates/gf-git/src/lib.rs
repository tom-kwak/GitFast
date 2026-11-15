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
#[cfg(feature = "gix")]
mod gix_reader;
pub mod ops;
mod repository;

pub use cli::{GitCommand, GitOutput, GitRunner, Progress, RunOptions};
pub use ops::diff::{DiffOptions, DiffTarget};
pub use ops::log::{LogOrder, LogQuery};
pub use repository::Repository;
pub use tokio_util::sync::CancellationToken;
