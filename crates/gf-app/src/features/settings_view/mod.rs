//! App-wide preferences, grouped into sections by what they affect.
//! ACCOUNTS lists the git servers logged in to (the tokens are kept by
//! git's credential helper) with log in / log out.

use gf_config::Account;
use iced::widget::{Column, button, column, container, row, space};
use iced::{Alignment, Element, Length, Padding};

use crate::theme::{self, Tokens, style};
use crate::widgets::{self, colored, dim, mono};

#[derive(Debug, Clone)]
pub enum Message {
    SetTheme(&'static str),
    /// Open the log-in dialog.
    LogIn,
    /// Log out of `accounts[i]` (asks first).
    LogOut(usize),
    Close,
}

/// Shows the app version, current theme, built-in theme previews and the
/// git servers logged in to.
pub fn view<'a>(t: &'a Tokens, accounts: &'a [Account]) -> Element<'a, Message> {
    let header = row![
        colored(t, "Settings", t.accent()),
        dim(t, concat!("v", env!("CARGO_PKG_VERSION"))),
        space().width(Length::Fill),
        widgets::text_button(t, "Done", Some("Esc"), Some(Message::Close)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let appearance = column![
        dim(t, "─ APPEARANCE"),
        row![
            mono(t, "Theme"),
            space().width(Length::Fill),
            dim(t, "↑/↓ to change"),
        ],
        Column::with_children(theme::PRESET_NAMES.iter().map(|&name| theme_row(t, name)))
            .spacing(2),
    ]
    .spacing(6);

    container(column![header, appearance, accounts_section(t, accounts)].spacing(14))
        .padding(Padding::from([12, 16]))
        .width(Length::Fill)
        .max_width(480)
        .style(style::accent_panel(t, t.accent()))
        .into()
}

fn accounts_section<'a>(t: &'a Tokens, accounts: &'a [Account]) -> Element<'a, Message> {
    let rows = accounts.iter().enumerate().map(|(i, a)| {
        let server = if a.protocol == "https" {
            a.host.clone()
        } else {
            format!("{}://{}", a.protocol, a.host)
        };
        row![
            colored(t, "●", t.added()),
            mono(t, server),
            dim(t, a.username.as_str()),
            space().width(Length::Fill),
            widgets::danger_button(t, "Log out", Some(Message::LogOut(i))),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    });
    let empty = accounts.is_empty().then(|| {
        dim(
            t,
            "Not logged in to any server. SSH remotes use your SSH keys.",
        )
        .into()
    });
    column![
        row![
            dim(t, "─ ACCOUNTS"),
            space().width(Length::Fill),
            widgets::text_button(t, "+ Log in", None, Some(Message::LogIn)),
        ]
        .align_y(Alignment::Center),
        Column::with_children(rows.chain(empty)).spacing(4),
        dim(
            t,
            "HTTPS tokens are kept by git's credential helper (Keychain, Credential Manager)."
        ),
    ]
    .spacing(6)
    .into()
}

fn theme_row<'a>(t: &'a Tokens, name: &'static str) -> Element<'a, Message> {
    let selected = t.name == name;
    let preview = theme::preset(name);
    let swatches = [preview.bg, preview.fg, preview.accent(), preview.added()]
        .into_iter()
        .fold(iced::widget::Row::new().spacing(2), |swatches, color| {
            swatches.push(
                container(space().width(12).height(12))
                    .style(move |_| container::Style::default().background(color)),
            )
        });
    let content = row![
        colored(
            t,
            if selected { "●" } else { "○" },
            if selected { t.accent() } else { t.dim }
        ),
        mono(t, name),
        space().width(Length::Fill),
        swatches,
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    button(content)
        .width(Length::Fill)
        .padding(Padding::from([5, 8]))
        .style(style::row_button(t, selected))
        .on_press_maybe((!selected).then_some(Message::SetTheme(name)))
        .into()
}
