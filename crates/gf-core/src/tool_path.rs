//! Where gf looks for `git` and the tools git runs (git-lfs, credential
//! helpers, editors).
//!
//! An app opened from Finder or the Dock gets the bare system `PATH`
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), not the shell's, so Homebrew's git,
//! git-lfs or `gh` would not be found. gf runs git with its own `PATH` plus
//! the usual package-manager folders it lacks. Computed once.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Folders added after the inherited `PATH` when missing from it.
const EXTRA: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

/// The `PATH` to run git with.
pub fn tool_path() -> &'static OsStr {
    static PATH: OnceLock<OsString> = OnceLock::new();
    PATH.get_or_init(|| with_extra(std::env::var_os("PATH")))
}

fn with_extra(path: Option<OsString>) -> OsString {
    let mut dirs: Vec<PathBuf> = path
        .as_deref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    for extra in EXTRA {
        if !dirs.iter().any(|d| d.as_os_str() == *extra) {
            dirs.push(PathBuf::from(extra));
        }
    }
    // Only colons inside a folder name fail here; keep what was inherited.
    std::env::join_paths(&dirs).unwrap_or_else(|_| path.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_folders_are_added_after_the_inherited_ones() {
        let p = with_extra(Some("/usr/bin:/bin".into()));
        assert_eq!(p, "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin");
        let p = with_extra(Some("/usr/local/bin:/opt/homebrew/bin:/usr/bin".into()));
        assert_eq!(p, "/usr/local/bin:/opt/homebrew/bin:/usr/bin");
        assert_eq!(with_extra(None), "/opt/homebrew/bin:/usr/local/bin");
    }
}
