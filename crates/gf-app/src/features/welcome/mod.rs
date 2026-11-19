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
}

pub fn view<'a>(t: &'a Tokens, recent: &'a [PathBuf], drop_hover: bool) -> Element<'a, Message> {
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

    let shortcut = |k: &'a str, what: &'a str| {
        row![colored(t, format!("{k:>6}"), t.accent()), dim(t, what)].spacing(10)
    };
    let drop_line = if drop_hover {
        colored(t, "▼ drop the folder to open it", t.success())
    } else {
        dim(t, "or drag a repository folder onto this window")
    };

    let content = column![
        row![logo::view(t), dim(t, "Git Fast · Built with Rust")]
            .spacing(16)
            .align_y(iced::Alignment::Center),
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
        shortcut("⌘O", "open repository"),
        shortcut("⌘1-9", "switch tab"),
        shortcut("⌘W", "close tab"),
        shortcut("⌘R", "refresh"),
        shortcut("⌘L", "focus the prompt  (↑/↓ history, ⇥ complete)"),
        shortcut("⌘J", "show/hide command log"),
        shortcut("?", "help (at the prompt)"),
    ]
    .spacing(6)
    .max_width(900);

    widgets::scroll(
        container(content)
            .padding(Padding::from([24, 32]))
            .width(Length::Fill),
    )
}
