use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use gf_core::{GfError, Result};

// Placeholder identity until the app name / bundle id is decided (PLAN.md §10).
// Changing these moves where settings and logs are stored.
pub const APP_QUALIFIER: &str = "dev";
pub const APP_ORGANIZATION: &str = "gf";
pub const APP_NAME: &str = "gf";

/// Where gf stores its files, following each OS's conventions:
///
/// | | macOS | Linux | Windows |
/// |---|---|---|---|
/// | config | `~/Library/Application Support/dev.gf.gf` | `~/.config/gf` | `%APPDATA%\gf\gf\config` |
/// | data | `~/Library/Application Support/dev.gf.gf` | `~/.local/share/gf` | `%APPDATA%\gf\gf\data` |
/// | cache | `~/Library/Caches/dev.gf.gf` | `~/.cache/gf` | `%LOCALAPPDATA%\gf\gf\cache` |
#[derive(Debug, Clone)]
pub struct AppPaths {
    config_dir: PathBuf,
    data_dir: PathBuf,
    cache_dir: PathBuf,
}

impl AppPaths {
    /// Resolves the per-user directories for this OS. Does not create them.
    pub fn resolve() -> Result<Self> {
        let dirs = ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_NAME)
            .ok_or_else(|| GfError::Other("could not determine home directory".into()))?;
        Ok(Self {
            config_dir: dirs.config_dir().to_path_buf(),
            data_dir: dirs.data_dir().to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
        })
    }

    /// Roots everything under one directory (tests, portable mode).
    pub fn under(root: &Path) -> Self {
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }

    /// Creates every directory that does not exist yet.
    pub fn ensure_exist(&self) -> Result<()> {
        for dir in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.log_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_on_this_machine() {
        assert!(AppPaths::resolve().is_ok());
    }

    #[test]
    fn under_root_layout() {
        let paths = AppPaths::under(Path::new("/tmp/gf-test"));
        assert_eq!(paths.log_dir(), Path::new("/tmp/gf-test/data/logs"));
        assert_eq!(
            paths.settings_file(),
            Path::new("/tmp/gf-test/config/settings.toml")
        );
    }
}
