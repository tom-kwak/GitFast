//! Execution of the system `git` executable.

mod command;
pub mod parse;
mod progress;
mod runner;

pub use command::GitCommand;
pub use progress::Progress;
pub use runner::{GitOutput, GitRunner, RunOptions};
