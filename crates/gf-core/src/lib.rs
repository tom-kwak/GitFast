//! Shared domain model, errors and event bus for gf.
//!
//! This crate must stay free of UI, process and network dependencies so every
//! other crate (and any future UI) can depend on it.

pub mod error;
pub mod event;
pub mod model;
pub mod tool_path;

pub use error::{GfError, Result};
pub use event::{CommandEvent, CommandPhase, CoreEvent, EventBus};
pub use tool_path::{git_program, set_git_program, tool_path};
