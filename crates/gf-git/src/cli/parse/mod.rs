//! Parsers for git's machine-readable output. Each parser is a pure
//! function over bytes, tested against literal git output, so they can be
//! reused by any caller (ops, the command runner UI, tests).
//!
//! Each format pairs with the exact flags that produce it; those flags live
//! next to the parser as `pub const ..._ARGS` so the two cannot drift apart.

pub mod diff;
pub mod log;
pub mod numstat;
pub mod refs;
pub mod remote;
pub mod status;
mod util;

pub use util::{bytes_to_path, unquote_c_path};
