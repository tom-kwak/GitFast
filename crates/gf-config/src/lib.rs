//! App settings, repository bookmarks/workspaces, keymaps and storage paths.

mod paths;
mod settings;

pub use paths::{APP_NAME, APP_ORGANIZATION, APP_QUALIFIER, AppPaths};
pub use settings::{
    AppearanceSettings, ConfirmSettings, DEFAULT_LIST_COLS, GitSettings, RepoSettings, Settings,
    WorktreeSettings,
};
