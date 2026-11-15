//! UI features, one folder each. Stateful features implement
//! [`crate::feature::Feature`]; pure views (tab bar, toolbar, status line,
//! welcome) are plain functions.

pub mod blame_view;
pub mod changes;
pub mod command_log;
pub mod commit_box;
pub mod conflict_view;
pub mod context_menu;
pub mod dialog;
pub mod diff_view;
pub mod history;
pub mod op_banner;
pub mod prompt;
pub mod rebase_editor;
pub mod sidebar;
pub mod status_line;
pub mod tabs;
pub mod toolbar;
pub mod welcome;
