//! UI features, one folder each. Stateful features implement
//! [`crate::feature::Feature`]; pure views (tab bar, status line, welcome)
//! are plain functions.

pub mod changes;
pub mod command_log;
pub mod history;
pub mod prompt;
pub mod sidebar;
pub mod status_line;
pub mod tabs;
pub mod welcome;
