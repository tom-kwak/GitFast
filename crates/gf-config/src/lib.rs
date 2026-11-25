//! App settings, repository bookmarks/workspaces, keymaps and storage paths.

mod paths;
mod settings;

pub use paths::{APP_NAME, APP_ORGANIZATION, APP_QUALIFIER, AppPaths};
pub use settings::{
    Account, AppearanceSettings, ConfirmSettings, DEFAULT_HISTORY_SHARE, DEFAULT_LIST_COLS,
    DEFAULT_REFS_COLS, GitSettings, RepoSettings, Settings, WorktreeSettings,
};
