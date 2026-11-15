//! High-level repository operations, grouped by area. Each file adds an
//! `impl Repository` block, so new areas (stash, rebase, submodule, ...) are
//! added as new files without touching existing ones.

pub mod diff;
pub mod log;
pub mod refs;
pub mod remote;
pub mod status;
