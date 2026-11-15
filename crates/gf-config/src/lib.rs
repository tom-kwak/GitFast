//! App settings, repository bookmarks/workspaces, keymaps and storage paths.
//!
//! Status: only [`AppPaths`] exists so far; settings persistence lands in M2.

mod paths;

pub use paths::{APP_NAME, APP_ORGANIZATION, APP_QUALIFIER, AppPaths};
