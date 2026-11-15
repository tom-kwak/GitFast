//! Reusable terminal-style building blocks. Features compose these instead
//! of styling raw iced widgets, so the look stays consistent.

pub mod format;

use iced::widget::{Text, button, column, container, row, scrollable, text, tooltip};
use iced::{Color, Element, Length, Padding};

use crate::theme::{Tokens, style};

/// Monospace text at the base size.
pub fn mono<'a>(t: &Tokens, s: impl text::IntoFragment<'a>) -> Text<'a> {
    text(s).size(t.font_size)
}

/// Monospace text in a color.
pub fn colored<'a>(t: &Tokens, s: impl text::IntoFragment<'a>, color: Color) -> Text<'a> {
    mono(t, s).color(color)
}

/// Secondary (dim) text.
pub fn dim<'a>(t: &Tokens, s: impl text::IntoFragment<'a>) -> Text<'a> {
    text(s).size(t.small()).color(t.dim)
}

/// `[ label ]` button. `shortcut` is shown inside the brackets so it is
/// discoverable without opening any help (`[ Open ⌘O ]`).
pub fn text_button<'a, M: Clone + 'a>(
    t: &'a Tokens,
    label: &str,
    shortcut: Option<&str>,
    on_press: Option<M>,
) -> Element<'a, M> {
    let label = match shortcut {
        Some(k) => format!("[ {label} {k} ]"),
        None => format!("[ {label} ]"),
    };
    let color = t.accent();
    button(mono(t, label))
        .padding(Padding::from([1, 2]))
        .style(style::text_button(t, color))
        .on_press_maybe(on_press)
        .into()
}

/// A whole-row clickable list item (large hit target, hover highlight).
pub fn list_row<'a, M: Clone + 'a>(
    t: &'a Tokens,
    content: impl Into<Element<'a, M>>,
    selected: bool,
    on_press: M,
) -> Element<'a, M> {
    button(content)
        .width(Length::Fill)
        .padding(Padding::from([1, 6]))
        .style(style::row_button(t, selected))
        .on_press(on_press)
        .into()
}

/// Wraps an element with a hover tooltip.
pub fn tip<'a, M: 'a>(
    t: &'a Tokens,
    content: impl Into<Element<'a, M>>,
    hint: String,
) -> Element<'a, M> {
    tooltip(
        content,
        container(dim(t, hint))
            .padding(Padding::from([2, 6]))
            .style(style::tooltip(t)),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

/// A titled, bordered panel: `─ TITLE ─────` header over its content.
pub fn panel<'a, M: 'a>(
    t: &'a Tokens,
    title: String,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    container(column![dim(t, format!("─ {title} ")), content.into()].spacing(4))
        .padding(Padding::from([4, 6]))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(style::panel(t))
        .into()
}

/// A vertical scroll area with a thin, terminal-like scrollbar.
pub fn scroll<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    scrollable(content)
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new().width(6).scroller_width(6),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// One-line hint shown in place of an empty list, telling the user what
/// would fill it (PLAN.md §4-2, "empty states guide").
pub fn empty_hint<'a, M: 'a>(t: &Tokens, hint: impl Into<String>) -> Element<'a, M> {
    container(dim(t, format!("  {}", hint.into())))
        .padding(Padding::from([4, 0]))
        .into()
}

/// Braille spinner frames for in-progress work.
pub const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner_frame(tick: usize) -> &'static str {
    SPINNER[tick % SPINNER.len()]
}

/// Label/value pair for the status line: `⎇ main`.
pub fn segment<'a, M: 'a>(
    t: &Tokens,
    icon: &'a str,
    value: String,
    color: Color,
) -> Element<'a, M> {
    row![dim(t, icon), colored(t, value, color)]
        .spacing(4)
        .into()
}
