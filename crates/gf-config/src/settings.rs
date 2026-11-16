//! User settings persisted as TOML (`settings.toml` in the config dir).
//!
//! Every field has a default and unknown keys are ignored, so files written
//! by older or newer versions of gf always load.

use std::path::{Path, PathBuf};

use gf_core::{GfError, Result};
use serde::{Deserialize, Serialize};

/// Maximum number of remembered recent repositories.
const MAX_RECENT: usize = 20;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: AppearanceSettings,
    pub confirm: ConfirmSettings,
    pub git: GitSettings,
    pub repos: RepoSettings,
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
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "gf-dark".into(),
            font_size: 13,
            show_command_log: true,
            show_refs: true,
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
        s.git.executable = Some("/opt/git/bin/git".into());
        s.repos.open = vec!["/a".into(), "/b".into()];
        s.repos.active = Some(1);
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
        for i in 0..25 {
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
