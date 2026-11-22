//! Empty state shown when no repository is open: how to open one, recent
//! repositories, and the most useful shortcuts (PLAN.md §4-2).

mod logo;

use std::path::PathBuf;

use iced::widget::{Column, column, container, row};
use iced::{Element, Length, Padding};

use crate::theme::Tokens;
use crate::widgets::{self, colored, dim, format, mono};

#[derive(Debug, Clone)]
pub enum Message {
    OpenRecent(PathBuf),
    /// The open-repository window.
    Open,
    /// Opens the git download page (git was not found).
    GetGit,
}

/// The shortcuts the welcome screen lists, in mac glyphs (drawn in the
/// platform's notation).
const SHORTCUTS: &[(&str, &str)] = &[
    ("⌘O", "open repository"),
    ("⌘1-9", "switch tab"),
    ("⌘W", "close tab"),
    ("⌘R", "refresh"),
    ("⌘L", "focus the prompt  (↑/↓ history, ⇥ complete)"),
    ("⌘J", "show/hide command log"),
    ("?", "help (at the prompt)"),
];

/// [`SHORTCUTS`], keys right-aligned in a column as wide as the longest
/// one as drawn (`⌘1-9` on macOS, `Ctrl+1-9` elsewhere).
fn shortcut_list<'a>(t: &'a Tokens) -> Column<'a, Message> {
    let keys: Vec<_> = SHORTCUTS
        .iter()
        .map(|(k, _)| crate::platform::keys::local((*k).into()))
        .collect();
    let width = keys.iter().map(|k| k.chars().count()).max().unwrap_or(0);
    let rows = keys.into_iter().zip(SHORTCUTS).map(|(k, (_, what))| {
        row![
            colored(t, format!("{k:>width$}"), t.accent()),
            dim(t, *what)
        ]
        .spacing(10)
        .into()
    });
    Column::with_children(rows).spacing(6)
}

/// What to do when git does not start: gf runs every command through it.
fn git_missing_banner<'a>(t: &'a Tokens) -> Element<'a, Message> {
    let git = gf_core::git_program().display().to_string();
    column![
        colored(t, format!("✗ git not found ({git})"), t.error()),
        row![
            widgets::text_button(t, "Download Git", None, Some(Message::GetGit)),
            dim(
                t,
                "then restart gf — or set git.executable in settings (⌘,) to git's full path"
            ),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(4)
    .into()
}

pub fn view<'a>(
    t: &'a Tokens,
    recent: &'a [PathBuf],
    drop_hover: bool,
    git_missing: bool,
) -> Element<'a, Message> {
    let mut recent_list = Column::new().spacing(1);
    if recent.is_empty() {
        recent_list = recent_list.push(widgets::empty_hint(
            t,
            "repositories you open will be listed here",
        ));
    }
    for (i, path) in recent.iter().take(10).enumerate() {
        let exists = path.exists();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let line = row![
            dim(t, format!("{:>2}", i + 1)),
            colored(t, name, if exists { t.branch() } else { t.dim })
                .width(Length::Fixed(t.font_size * 16.0)),
            dim(t, format::tilde(path)),
        ]
        .spacing(10);
        let row = widgets::list_row(t, line, false, Message::OpenRecent(path.clone()));
        recent_list = recent_list.push(if exists {
            row
        } else {
            widgets::tip(t, row, "folder not found — it may have moved".into())
        });
    }

    let drop_line = if drop_hover {
        colored(t, "▼ drop the folder to open it", t.success())
    } else {
        dim(t, "or drag a repository folder onto this window")
    };

    let banner: Element<'a, Message> = if git_missing {
        git_missing_banner(t)
    } else {
        column![].into()
    };
    let content = column![
        row![logo::view(t), dim(t, "Git Fast · Built with Rust")]
            .spacing(16)
            .align_y(iced::Alignment::Center),
        banner,
        row![
            widgets::text_button(t, "Open repository…", Some("⌘O"), Some(Message::Open)),
            drop_line,
        ]
        .spacing(16)
        .align_y(iced::Alignment::Center),
        dim(t, "type `:open ~/path/to/repo` or any git command (e.g. `clone <url>`) at the prompt below"),
        mono(t, ""),
        row![dim(t, "─ RECENT "), dim(t, "· click, or type its number + ⏎ at the prompt")].spacing(6),
        recent_list,
        mono(t, ""),
        dim(t, "─ SHORTCUTS "),
        shortcut_list(t),
    ]
    .spacing(6)
    .max_width(900);

    widgets::scroll(
        container(content)
            .padding(Padding::from([24, 32]))
            .width(Length::Fill),
    )
}
