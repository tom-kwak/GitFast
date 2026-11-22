//! Where gf looks for `git` and the tools git runs (git-lfs, credential
//! helpers, editors).
//!
//! An app opened from Finder or the Dock gets the bare system `PATH`
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), not the shell's, so Homebrew's git,
//! git-lfs or `gh` would not be found. On Windows, Git for Windows may be
//! installed without being added to `PATH`. gf runs git with its own `PATH`
//! plus the usual install folders it lacks. Computed once.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// `CREATE_NO_WINDOW`: a console child of a GUI app opens no console window.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Folders added after the inherited `PATH` when missing from it.
#[cfg(not(windows))]
fn extra_dirs() -> Vec<PathBuf> {
    ["/opt/homebrew/bin", "/usr/local/bin"]
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

/// Folders added after the inherited `PATH` when missing from it: where
/// the Git for Windows installer puts `git.exe` (machine-wide or per user).
#[cfg(windows)]
fn extra_dirs() -> Vec<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let mut dirs = Vec::new();
    for base in [var("ProgramFiles"), var("ProgramW6432")]
        .into_iter()
        .flatten()
    {
        dirs.push(base.join("Git").join("cmd"));
    }
    if let Some(local) = var("LOCALAPPDATA") {
        dirs.push(local.join("Programs").join("Git").join("cmd"));
    }
    dirs
}

/// The `PATH` to run git with.
pub fn tool_path() -> &'static OsStr {
    static PATH: OnceLock<OsString> = OnceLock::new();
    PATH.get_or_init(|| with_extra(std::env::var_os("PATH"), &extra_dirs()))
}

static GIT: OnceLock<PathBuf> = OnceLock::new();

/// Sets the git executable every git process uses (the `git.executable`
/// setting). Only the first call counts; call it before starting git.
pub fn set_git_program(git: impl Into<PathBuf>) {
    let _ = GIT.set(git.into());
}

/// The git executable to start: the configured one, else `git` from
/// [`tool_path`].
pub fn git_program() -> &'static Path {
    GIT.get().map_or(Path::new("git"), PathBuf::as_path)
}

/// Keeps a console program started by gf (git, mostly) from opening a
/// console window on Windows. Does nothing elsewhere.
pub fn hide_console(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// A blocking `git` command: [`git_program`] with gf's `PATH`, no console
/// window. For quick questions outside the runner (which is async).
pub fn git_command() -> std::process::Command {
    let mut cmd = std::process::Command::new(git_program());
    cmd.env("PATH", tool_path());
    hide_console(&mut cmd);
    cmd
}

/// Whether the git executable starts at all (`git --version`). Blocking.
pub fn git_available() -> bool {
    git_command()
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .is_ok_and(|out| out.status.success())
}

fn with_extra(path: Option<OsString>, extra: &[PathBuf]) -> OsString {
    let mut dirs: Vec<PathBuf> = path
        .as_deref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    for extra in extra {
        if !dirs.iter().any(|d| same_dir(d, extra)) {
            dirs.push(extra.clone());
        }
    }
    // Only separators inside a folder name fail here; keep what was inherited.
    std::env::join_paths(&dirs).unwrap_or_else(|_| path.unwrap_or_default())
}

#[cfg(not(windows))]
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
}

/// Windows paths ignore case and a trailing separator.
#[cfg(windows)]
fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn missing_folders_are_added_after_the_inherited_ones() {
        let extra = extra_dirs();
        let p = with_extra(Some("/usr/bin:/bin".into()), &extra);
        assert_eq!(p, "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin");
        let p = with_extra(
            Some("/usr/local/bin:/opt/homebrew/bin:/usr/bin".into()),
            &extra,
        );
        assert_eq!(p, "/usr/local/bin:/opt/homebrew/bin:/usr/bin");
        assert_eq!(with_extra(None, &extra), "/opt/homebrew/bin:/usr/local/bin");
    }

    #[cfg(windows)]
    #[test]
    fn missing_folders_are_added_after_the_inherited_ones() {
        let extra = [PathBuf::from(r"C:\Program Files\Git\cmd")];
        let p = with_extra(Some(r"C:\Windows;C:\bin".into()), &extra);
        assert_eq!(p, r"C:\Windows;C:\bin;C:\Program Files\Git\cmd");
        // Already there, in another case and with a trailing separator.
        let p = with_extra(Some(r"c:\program files\git\cmd\;C:\bin".into()), &extra);
        assert_eq!(p, r"c:\program files\git\cmd\;C:\bin");
        assert_eq!(with_extra(None, &extra), r"C:\Program Files\Git\cmd");
    }

    #[test]
    fn git_program_defaults_to_git() {
        // Tests never call set_git_program.
        assert_eq!(git_program(), Path::new("git"));
    }
}
