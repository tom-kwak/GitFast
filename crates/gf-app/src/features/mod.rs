//! UI features, one folder each. Stateful features implement
//! [`crate::feature::Feature`]; pure views (tab bar, toolbar, status line,
//! welcome) are plain functions.

pub mod blame_view;
pub mod changes;
pub mod code_view;
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
pub mod repo_picker;
pub mod settings_view;
pub mod sidebar;
pub mod status_line;
pub mod tab_list;
pub mod tab_switcher;
pub mod tabs;
pub mod toolbar;
pub mod undo_list;
pub mod welcome;
pub mod worktrees;
