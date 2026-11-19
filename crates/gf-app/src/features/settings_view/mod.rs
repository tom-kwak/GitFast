//! App-wide preferences, grouped into sections by what they affect.

use iced::widget::{Column, button, column, container, row, space};
use iced::{Alignment, Element, Length, Padding};

use crate::theme::{self, Tokens, style};
use crate::widgets::{self, colored, dim, mono};

#[derive(Debug, Clone)]
pub enum Message {
    SetTheme(&'static str),
    Close,
}

/// Shows the app version, current theme, and built-in theme previews.
pub fn view(t: &Tokens) -> Element<'_, Message> {
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

    container(column![header, appearance].spacing(14))
        .padding(Padding::from([12, 16]))
        .width(Length::Fill)
        .max_width(480)
        .style(style::accent_panel(t, t.accent()))
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
