//! User settings persisted as TOML (`settings.toml` in the config dir).
//!
//! Every field has a default and unknown keys are ignored, so files written
//! by older or newer versions of gf always load.

use std::path::{Path, PathBuf};

use gf_core::{GfError, Result};
use serde::{Deserialize, Serialize};

/// Maximum number of remembered recent repositories.
const MAX_RECENT: usize = 100;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: AppearanceSettings,
    pub confirm: ConfirmSettings,
    pub git: GitSettings,
    pub repos: RepoSettings,
    pub worktrees: WorktreeSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    /// Theme preset name (see `gf-app` theme presets).
    pub theme: String,
    /// Base font size in points.
    pub font_size: u16,
    /// Show the command log panel.
    pub show_command_log: bool,
    /// Show the refs sidebar (left).
    pub show_refs: bool,
    /// Show the commit list in History mode.
    pub show_history: bool,
    /// Width of the file list beside the diff, in character columns; the
    /// same in HISTORY and WORKING TREE. Dragged with the divider.
    pub list_cols: f32,
    /// Width of the refs panel (left), in character columns; the same in
    /// REFS and WORKTREES. Dragged with the divider.
    pub refs_cols: f32,
    /// Directory tree in the working tree's changes list.
    pub working_tree_dirs: bool,
    /// Directory tree in the history commit's file list.
    pub history_file_dirs: bool,
    /// The refs panel shows the worktrees view instead of the refs (⌘⇧B).
    pub worktrees_view: bool,
}

/// Default width of the file list beside the diff (character columns).
pub const DEFAULT_LIST_COLS: f32 = 34.0;

/// Default width of the refs panel (character columns).
pub const DEFAULT_REFS_COLS: f32 = 36.0;

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "gf-dark".into(),
            font_size: 13,
            show_command_log: false,
            show_refs: true,
            show_history: true,
            list_cols: DEFAULT_LIST_COLS,
            refs_cols: DEFAULT_REFS_COLS,
            working_tree_dirs: false,
            history_file_dirs: true,
            worktrees_view: false,
        }
    }
}

/// Confirmations the user chose to skip ("don't ask again"). Only
/// change-type confirmations can be skipped; deletions always ask.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfirmSettings {
    pub skip: Vec<String>,
}

impl ConfirmSettings {
    pub fn is_skipped(&self, key: &str) -> bool {
        self.skip.iter().any(|k| k == key)
    }

    pub fn skip(&mut self, key: &str) {
        if !self.is_skipped(key) {
            self.skip.push(key.to_owned());
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitSettings {
    /// Explicit git executable; `None` uses `git` from `PATH`.
    pub executable: Option<PathBuf>,
    /// Fetch in-process with gix instead of `git fetch` (falls back to git
    /// for anything gix does not handle). Off by default; `:gix-fetch on`.
    pub gix_fetch: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoSettings {
    /// Most recently opened first.
    pub recent: Vec<PathBuf>,
    /// Tabs open when the app last quit, restored on start.
    pub open: Vec<PathBuf>,
    /// Index into `open` of the active tab.
    pub active: Option<usize>,
}

/// The worktrees view (refs panel, ⌘⇧B): how to open a tree elsewhere
/// and where new trees go.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreeSettings {
    /// Command that opens a folder in the IDE (`code`, `cursor`, `idea`…);
    /// the folder is its last argument. `:editor <cmd>` changes it.
    pub editor: String,
    /// Terminal application; `None` uses the system's own. `:terminal`
    /// changes it.
    pub terminal: Option<String>,
    /// Folder new worktrees go in; `None` puts them in
    /// `<repo>-worktrees/` beside the repository.
    pub root: Option<PathBuf>,
    /// Seconds between status reads of the other trees while the view is
    /// shown (their files are not watched).
    pub poll_secs: u64,
}

impl Default for WorktreeSettings {
    fn default() -> Self {
        Self {
            editor: "code".into(),
            terminal: None,
            root: None,
            poll_secs: 5,
        }
    }
}

impl Settings {
    /// Loads settings; a missing file yields defaults.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| GfError::parse("settings", format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes atomically (temp file + rename) so a crash never leaves a
    /// half-written settings file behind.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)
            .map_err(|e| GfError::Other(format!("serializing settings: {e}")))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Moves `path` to the front of the recent list.
    pub fn remember_repo(&mut self, path: &Path) {
        let recent = &mut self.repos.recent;
        recent.retain(|p| p != path);
        recent.insert(0, path.to_path_buf());
        recent.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = Settings::load(&dir.path().join("nope.toml")).expect("load");
        assert_eq!(s, Settings::default());
        assert_eq!(s.appearance.theme, "gf-dark");
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cfg/settings.toml");
        let mut s = Settings::default();
        s.appearance.font_size = 15;
        s.appearance.show_command_log = true;
        s.appearance.show_history = false;
        s.git.executable = Some("/opt/git/bin/git".into());
        s.repos.open = vec!["/a".into(), "/b".into()];
        s.repos.active = Some(1);
        s.worktrees.editor = "cursor".into();
        s.worktrees.root = Some("/w".into());
        s.save(&path).expect("save");
        assert_eq!(Settings::load(&path).expect("load"), s);
    }

    #[test]
    fn partial_and_unknown_keys_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "[appearance]\nfont_size = 16\nfuture_key = 1\n[unknown]\nx = 2\n",
        )
        .expect("write");
        let s = Settings::load(&path).expect("load");
        assert_eq!(s.appearance.font_size, 16);
        assert!(s.appearance.show_history);
        assert!(!s.appearance.show_command_log);
        assert_eq!(s.appearance.theme, "gf-dark");
    }

    #[test]
    fn invalid_file_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "appearance = 3").expect("write");
        assert!(Settings::load(&path).is_err());
    }

    #[test]
    fn recent_repos_are_deduplicated_and_capped() {
        let mut s = Settings::default();
        for i in 0..MAX_RECENT + 5 {
            s.remember_repo(Path::new(&format!("/r{i}")));
        }
        s.remember_repo(Path::new("/r10"));
        assert_eq!(s.repos.recent.len(), MAX_RECENT);
        assert_eq!(s.repos.recent[0], Path::new("/r10"));
        assert_eq!(
            s.repos
                .recent
                .iter()
                .filter(|p| *p == Path::new("/r10"))
                .count(),
            1
        );
    }

    #[test]
    fn skipped_confirmations_are_unique() {
        let mut c = ConfirmSettings::default();
        c.skip("pull");
        c.skip("pull");
        assert!(c.is_skipped("pull"));
        assert!(!c.is_skipped("push"));
        assert_eq!(c.skip.len(), 1);
    }
}
