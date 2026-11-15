//! UI features, one folder each. Stateful features implement
//! [`crate::feature::Feature`]; pure views (tab bar, toolbar, status line,
//! welcome) are plain functions.

pub mod changes;
pub mod command_log;
pub mod commit_box;
pub mod dialog;
pub mod diff_view;
pub mod history;
pub mod prompt;
pub mod sidebar;
pub mod status_line;
pub mod tabs;
pub mod toolbar;
pub mod welcome;
