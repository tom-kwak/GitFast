//! Everything that differs between operating systems, in one place.
//!
//! Features call these functions instead of writing `cfg!(target_os = …)`
//! themselves, so supporting a platform means reading this folder only.
//! macOS behaviour is the reference: on macOS every function here returns
//! what gf did before platform support existed.
//!
//! - [`keys`]: shortcut labels (`⌘⇧K` on macOS, `Ctrl+Shift+K` elsewhere)
//! - [`home_dir`], [`is_path_sep`]: user paths
//! - [`FILE_MANAGER`], [`reveal`]: showing a file in the file manager
//! - [`looks_like_path`]: typed text that names a folder (`C:\x` on Windows)
//! - [`mono_font`], [`window_settings`]: the font, the main window (icon)
//! - [`open_url`], [`GIT_DOWNLOAD_URL`]: the browser, where to get git
//! - [`open_in_editor`], [`open_terminal`], [`TERMINAL`]: a folder in the
//!   IDE or a terminal (worktrees view)

pub mod keys;

use std::path::{Path, PathBuf};

/// The file manager's name, for menu items ("reveal in Finder").
pub const FILE_MANAGER: &str = if cfg!(target_os = "macos") {
    "Finder"
} else if cfg!(windows) {
    "File Explorer"
} else {
    "file manager"
};

/// The user's home folder: `HOME`, or `USERPROFILE` on Windows (which
/// usually has no `HOME`).
pub fn home_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty());
    #[cfg(windows)]
    let home = home.or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()));
    home.map(PathBuf::from)
}

/// The fixed-width font gf draws everything in.
///
/// `Font::MONOSPACE` is the system's monospace family: Menlo on macOS, but
/// Courier New on Windows (thin and hard to read). Windows gets Cascadia
/// Mono (Windows 11, Windows Terminal), else Consolas (every Windows).
pub fn mono_font() -> iced::Font {
    #[cfg(windows)]
    {
        static FONT: std::sync::OnceLock<iced::Font> = std::sync::OnceLock::new();
        *FONT.get_or_init(|| {
            let fonts = std::env::var_os("WINDIR")
                .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
                .join("Fonts");
            let user_fonts = std::env::var_os("LOCALAPPDATA")
                .map(|d| PathBuf::from(d).join(r"Microsoft\Windows\Fonts"));
            let has_cascadia = std::iter::once(fonts)
                .chain(user_fonts)
                .any(|dir| dir.join("CascadiaMono.ttf").exists());
            iced::Font::with_name(if has_cascadia {
                "Cascadia Mono"
            } else {
                "Consolas"
            })
        })
    }
    #[cfg(not(windows))]
    iced::Font::MONOSPACE
}

/// The main window: size, position, and on Windows/Linux the window and
/// taskbar icon (macOS takes it from the .app bundle).
pub fn window_settings() -> iced::window::Settings {
    let icon = if cfg!(target_os = "macos") {
        None
    } else {
        iced::window::icon::from_rgba(
            include_bytes!("../../assets/gf-icon-64.rgba").to_vec(),
            64,
            64,
        )
        .ok()
    };
    iced::window::Settings {
        size: iced::Size::new(1280.0, 820.0),
        position: iced::window::Position::Centered,
        icon,
        ..iced::window::Settings::default()
    }
}

/// Whether `c` separates folders in a path the user typed. Windows takes
/// both `\` and `/`.
pub fn is_path_sep(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// `path` with symlinks resolved, in the form git and the tabs use: on
/// Windows without the `\\?\` prefix `std::fs::canonicalize` adds (which
/// would never equal `C:\x`). Blocking: call it off the UI thread.
pub fn canonicalize(path: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(path).ok()?;
    Some(without_verbatim(real))
}

/// `path` without Windows' verbatim prefix (`\\?\C:\x` → `C:\x`); UNC
/// paths and other platforms are left as they are.
fn without_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if cfg!(windows) && !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

/// Whether typed text is a path rather than a name to search for:
/// `/x`, `~`, and on Windows also `C:\x`, `C:/x` and `\\server\share`.
pub fn looks_like_path(query: &str) -> bool {
    if query.starts_with('/') || query.starts_with('~') {
        return true;
    }
    if !cfg!(windows) {
        return false;
    }
    let mut chars = query.chars();
    query.starts_with('\\')
        || matches!(
            (chars.next(), chars.next(), chars.next()),
            (Some(d), Some(':'), None | Some('\\' | '/')) if d.is_ascii_alphabetic()
        )
}

/// Where to get git for this platform.
pub const GIT_DOWNLOAD_URL: &str = if cfg!(windows) {
    "https://git-scm.com/download/win"
} else if cfg!(target_os = "macos") {
    "https://git-scm.com/download/mac"
} else {
    "https://git-scm.com/download/linux"
};

/// Opens a web page in the default browser. Does not wait for it.
pub fn open_url(url: &str) -> std::io::Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        // `explorer <url>` hands it to the default browser, no console.
        std::process::Command::new("explorer")
    } else {
        std::process::Command::new("xdg-open")
    };
    let mut child = cmd.arg(url).spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// The credential helper to offer when git has none, so logins are
/// kept: (`credential.helper` value, what the user knows it as). Linux
/// has no helper every system ships.
pub const CREDENTIAL_HELPER: Option<(&str, &str)> = if cfg!(target_os = "macos") {
    Some(("osxkeychain", "macOS Keychain"))
} else if cfg!(windows) {
    Some(("manager", "Git Credential Manager"))
} else {
    None
};

/// The system terminal's name, for menu items ("open in Terminal").
pub const TERMINAL: &str = if cfg!(target_os = "macos") {
    "Terminal"
} else if cfg!(windows) {
    "Windows Terminal"
} else {
    "terminal"
};

/// Starts `cmd` without waiting, reaping it in the background.
fn spawn_detached(mut cmd: std::process::Command) -> std::io::Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    cmd.env("PATH", gf_core::tool_path::tool_path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Splits a configured command (`code -n`) into program and arguments.
fn command_line(command: &str) -> Option<std::process::Command> {
    let mut words = command.split_whitespace();
    let mut cmd = std::process::Command::new(words.next()?);
    cmd.args(words);
    Some(cmd)
}

/// Opens the folder `dir` with the IDE `command` (`code`, `cursor`,
/// `idea`…; the folder is its last argument). Does not wait for it.
pub fn open_in_editor(command: &str, dir: &Path) -> std::io::Result<()> {
    std::fs::metadata(dir)?;
    let missing = || std::io::Error::new(std::io::ErrorKind::InvalidInput, "no editor command");
    if cfg!(windows) {
        // IDE launchers are `.cmd` scripts there, which only cmd starts.
        let mut cmd = std::process::Command::new("cmd");
        cmd.arg("/C").args(command.split_whitespace()).arg(dir);
        gf_core::tool_path::hide_console(&mut cmd);
        return spawn_detached(cmd);
    }
    let mut cmd = command_line(command).ok_or_else(missing)?;
    cmd.arg(dir);
    match spawn_detached(cmd) {
        // Apps started from Finder often lack the IDE's shell command:
        // ask macOS for an application of that name instead.
        Err(e) if cfg!(target_os = "macos") && e.kind() == std::io::ErrorKind::NotFound => {
            let mut open = std::process::Command::new("open");
            open.arg("-a").arg(command).arg(dir);
            spawn_detached(open)
        }
        other => other,
    }
}

/// Opens a terminal in `dir`: `app` (an application name or command), or
/// the system's terminal. Does not wait for it.
pub fn open_terminal(app: Option<&str>, dir: &Path) -> std::io::Result<()> {
    std::fs::metadata(dir)?;
    if cfg!(target_os = "macos") {
        let mut open = std::process::Command::new("open");
        open.arg("-a").arg(app.unwrap_or("Terminal")).arg(dir);
        return spawn_detached(open);
    }
    if cfg!(windows) {
        let mut cmd = std::process::Command::new("cmd");
        match app {
            Some(app) => cmd.arg("/C").args(app.split_whitespace()),
            // Windows Terminal when installed, else a console window.
            None => cmd.args(["/C", "start", "", "/D"]).arg(dir).arg("wt"),
        };
        cmd.current_dir(dir);
        gf_core::tool_path::hide_console(&mut cmd);
        return spawn_detached(cmd);
    }
    let mut cmd = command_line(app.unwrap_or("x-terminal-emulator"))
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no terminal"))?;
    cmd.current_dir(dir);
    spawn_detached(cmd)
}

/// Shows `path` selected in the file manager. Does not wait for it.
pub fn reveal(path: &Path) -> std::io::Result<()> {
    // Historical paths may have been removed from the working tree.
    std::fs::symlink_metadata(path)?;
    if cfg!(test) {
        return Ok(());
    }
    let mut child = reveal_command(path).spawn()?;
    // Reaped in the background so it never lingers as a zombie. Explorer
    // exits with 1 even when it worked, so the exit code is not checked.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(target_os = "macos")]
fn reveal_command(path: &Path) -> std::process::Command {
    let mut c = std::process::Command::new("open");
    c.arg("-R").arg(path);
    c
}

#[cfg(windows)]
fn reveal_command(path: &Path) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new("explorer");
    c.raw_arg(explorer_select_arg(path));
    c
}

/// `/select,"C:\a b\x,y.txt"`. Explorer splits its own arguments at commas
/// and does not undo the quoting Rust would add around the whole argument,
/// so the path is quoted by hand (a Windows path cannot contain `"`).
/// Explorer only understands `\`, and paths from git use `/`.
#[cfg(any(windows, test))]
fn explorer_select_arg(path: &Path) -> std::ffi::OsString {
    let native: PathBuf = path.components().collect();
    let mut arg = std::ffi::OsString::from("/select,\"");
    arg.push(native.as_os_str());
    arg.push("\"");
    arg
}

#[cfg(not(any(target_os = "macos", windows)))]
fn reveal_command(path: &Path) -> std::process::Command {
    let mut c = std::process::Command::new("xdg-open");
    c.arg(path.parent().unwrap_or(path));
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_separators() {
        assert!(is_path_sep('/'));
        assert_eq!(is_path_sep('\\'), cfg!(windows));
        assert!(!is_path_sep('a'));
    }

    #[test]
    fn paths_are_told_from_names() {
        assert!(looks_like_path("/Users"));
        assert!(looks_like_path("~/work"));
        assert!(!looks_like_path("gf"));
        assert!(!looks_like_path("a:b"));
        assert_eq!(looks_like_path(r"C:\src"), cfg!(windows));
        assert_eq!(looks_like_path("c:/src"), cfg!(windows));
        assert_eq!(looks_like_path("D:"), cfg!(windows));
        assert_eq!(looks_like_path(r"\\server\share"), cfg!(windows));
    }

    #[test]
    fn explorer_gets_the_path_quoted_after_select() {
        let arg = explorer_select_arg(Path::new("C:/작업 폴더/a,b.txt"));
        if cfg!(windows) {
            assert_eq!(arg, r#"/select,"C:\작업 폴더\a,b.txt""#);
        } else {
            assert_eq!(arg, r#"/select,"C:/작업 폴더/a,b.txt""#);
        }
    }

    #[test]
    fn file_manager_name() {
        #[cfg(target_os = "macos")]
        assert_eq!(FILE_MANAGER, "Finder");
        #[cfg(windows)]
        assert_eq!(FILE_MANAGER, "File Explorer");
    }
}
