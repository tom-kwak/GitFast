//! Reusable terminal-style building blocks. Features compose these instead
//! of styling raw iced widgets, so the look stays consistent.

pub mod file_search;
pub mod file_tree;
pub mod format;
pub mod graph_canvas;
pub mod memo;
pub mod pointer;
pub mod selectable_line;
pub mod splitter;
pub mod virtual_list;

use iced::widget::{Text, button, column, container, row, scrollable, text, tooltip};
use iced::{Color, Element, Length, Padding};

use crate::theme::{Tokens, style};

pub(crate) const PUSH_ARROW: &str = "⇡";

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

/// `[ label ]` button drawn in the error color, for destructive actions.
pub fn danger_button<'a, M: Clone + 'a>(
    t: &'a Tokens,
    label: &str,
    on_press: Option<M>,
) -> Element<'a, M> {
    button(mono(t, format!("[ {label} ]")))
        .padding(Padding::from([1, 2]))
        .style(style::text_button(t, t.error()))
        .on_press_maybe(on_press)
        .into()
}

/// Small one-glyph action button with a tooltip explaining it (row actions
/// such as stage `+` / unstage `−` / discard `↺`).
pub fn icon_button<'a, M: Clone + 'a>(
    t: &'a Tokens,
    glyph: &'a str,
    color: Color,
    hint: impl Into<String>,
    on_press: M,
) -> Element<'a, M> {
    icon_button_maybe(t, glyph, color, hint, Some(on_press))
}

/// [`icon_button`] that is disabled (dimmed, not clickable) without a
/// message, e.g. "move up" on the first row. It keeps its place so the
/// rows stay aligned.
pub fn icon_button_maybe<'a, M: Clone + 'a>(
    t: &'a Tokens,
    glyph: &'a str,
    color: Color,
    hint: impl Into<String>,
    on_press: Option<M>,
) -> Element<'a, M> {
    let color = if on_press.is_some() { color } else { t.dim };
    tip(
        t,
        button(mono(t, glyph))
            .padding(Padding::from([1, 5]))
            .style(style::text_button(t, color))
            .on_press_maybe(on_press),
        hint.into(),
    )
}

/// Like [`list_row`] but without vertical padding and filling the row
/// height, so content such as graph lines can reach the row edges.
pub fn list_row_flush<'a, M: Clone + 'a>(
    t: &'a Tokens,
    content: impl Into<Element<'a, M>>,
    selected: bool,
    on_press: M,
) -> Element<'a, M> {
    button(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Padding::from([0, 6]))
        .style(style::row_button(t, selected))
        .on_press(on_press)
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

/// Like [`tip`], shown to the right (menus, where below would cover the next entry).
pub fn tip_right<'a, M: 'a>(
    t: &'a Tokens,
    content: impl Into<Element<'a, M>>,
    hint: String,
) -> Element<'a, M> {
    if hint.is_empty() {
        return content.into();
    }
    tooltip(
        content,
        container(dim(t, hint))
            .padding(Padding::from([2, 6]))
            .style(style::tooltip(t)),
        tooltip::Position::Right,
    )
    .gap(6)
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

/// Like [`tip`], but shown at the mouse and moving with it. For wide rows
/// (a line of code), where "below the middle" lands far from the pointer.
pub fn tip_at_cursor<'a, M: 'a>(
    t: &'a Tokens,
    content: impl Into<Element<'a, M>>,
    hint: String,
) -> Element<'a, M> {
    tooltip(
        content,
        container(dim(t, hint))
            .padding(Padding::from([2, 6]))
            .style(style::tooltip(t)),
        tooltip::Position::FollowCursor,
    )
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

/// [`panel`] with a control at the right end of its title line (e.g. the
/// section's own collapse button).
pub fn panel_with_action<'a, M: 'a>(
    t: &'a Tokens,
    title: String,
    action: impl Into<Element<'a, M>>,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    let header = row![
        dim(t, format!("─ {title} ")).width(Length::Fill),
        action.into()
    ]
    .align_y(iced::Alignment::Center);
    container(column![header, content.into()].spacing(4))
        .padding(Padding::from([4, 6]))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(style::panel(t))
        .into()
}

/// [`panel`] with a colored border and title, e.g. `━ LOCAL CHANGES`.
pub fn accent_panel<'a, M: 'a>(
    t: &'a Tokens,
    title: String,
    accent: Color,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    container(
        column![
            colored(t, format!("━ {title} "), accent).size(t.small()),
            content.into()
        ]
        .spacing(4),
    )
    .padding(Padding::from([4, 6]))
    .width(Length::Fill)
    .height(Length::Fill)
    .style(style::accent_panel(t, accent))
    .into()
}

/// Accent panel with a compact control beside the title.
pub fn accent_panel_with_action<'a, M: 'a>(
    t: &'a Tokens,
    title: String,
    accent: Color,
    action: Element<'a, M>,
    content: Element<'a, M>,
) -> Element<'a, M> {
    let header = row![
        colored(t, format!("━ {title} "), accent)
            .size(t.small())
            .width(Length::Fill),
        action,
    ]
    .align_y(iced::Alignment::Center);
    container(column![header, content].spacing(4))
        .padding(Padding::from([4, 6]))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(style::accent_panel(t, accent))
        .into()
}

/// Thin full-height strip standing in for a closed side panel: an arrow
/// and the panel name written vertically; clicking it reopens the panel.
pub fn collapsed_strip<'a, M: Clone + 'a>(
    t: &'a Tokens,
    arrow: &'a str,
    label: &str,
    hint: String,
    on_press: M,
) -> Element<'a, M> {
    let mut col = column![colored(t, arrow, t.accent())]
        .spacing(0)
        .align_x(iced::Alignment::Center);
    for ch in label.chars() {
        col = col.push(dim(t, ch.to_string()));
    }
    tip(
        t,
        button(col)
            .width(Length::Fixed(t.font_size * 1.8))
            .height(Length::Fill)
            .padding(Padding::from([6, 2]))
            .style(style::row_button(t, false))
            .on_press(on_press),
        hint,
    )
}

/// Full-width button standing in for a closed panel above or below the content.
pub fn collapsed_bar<'a, M: Clone + 'a>(
    t: &'a Tokens,
    content: impl Into<Element<'a, M>>,
    hint: &str,
    on_press: M,
) -> Element<'a, M> {
    tip(
        t,
        button(content)
            .clip(true)
            .width(Length::Fill)
            .padding(Padding::from([2, 6]))
            .style(style::row_button(t, false))
            .on_press(on_press),
        hint.into(),
    )
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

/// Line counts for [`file_entry`]: `None` = unknown (nothing shown),
/// `Some(None)` = binary, `Some(Some(c))` = `+added -removed`.
pub type EntryCounts = Option<Option<gf_git::LineCounts>>;

fn counts_text(counts: EntryCounts) -> Option<(String, String)> {
    match counts? {
        Some(c) => Some((format!("+{}", c.added), format!("-{}", c.removed))),
        None => Some(("bin".into(), String::new())),
    }
}

/// One file in a list, the same everywhere (commit files, working tree):
/// `M  main.rs  +3 -1  src/app` — status code, file name, line counts,
/// then the folder dimmed.
/// Color of a one-letter change code (`A`, `M`, `D`, ...), the same in
/// the working tree and in commit file lists.
pub fn change_color(t: &Tokens, code: char) -> Color {
    match code {
        'A' | '?' => t.added(),
        'D' => t.removed(),
        'M' | 'T' => t.warning(),
        'R' | 'C' => t.accent(),
        'U' => t.error(),
        _ => t.dim,
    }
}

pub(crate) fn file_name_label(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

pub fn file_entry<'a, M: 'a>(
    t: &Tokens,
    code: &str,
    code_color: Color,
    path: &std::path::Path,
    counts: EntryCounts,
) -> iced::widget::Row<'a, M> {
    row![
        colored(t, code.to_owned(), code_color),
        file_entry_details(t, file_name_label(path), path.parent(), counts),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
}

/// Name and change counts, placed after the status and any tree indentation.
pub fn file_entry_details<'a, M: 'a>(
    t: &Tokens,
    name: String,
    directory: Option<&std::path::Path>,
    counts: EntryCounts,
) -> iced::widget::Row<'a, M> {
    use iced::widget::text::Wrapping;
    let mut line = row![mono(t, name).wrapping(Wrapping::None)]
        .spacing(6)
        .align_y(iced::Alignment::Center);
    match counts_text(counts) {
        Some((added, removed)) if removed.is_empty() => {
            line = line.push(dim(t, added).wrapping(Wrapping::None));
        }
        Some((added, removed)) => {
            line = line.push(
                row![
                    colored(t, added, t.added()).wrapping(Wrapping::None),
                    colored(t, removed, t.removed()).wrapping(Wrapping::None),
                ]
                .spacing(4),
            );
        }
        None => {}
    }
    if let Some(dir) = directory.filter(|d| !d.as_os_str().is_empty()) {
        line = line.push(dim(t, dir.display().to_string()).wrapping(Wrapping::None));
    }
    line
}

/// Monospace columns [`file_entry`] needs (for horizontal scrolling).
pub fn file_entry_columns(path: &std::path::Path, counts: EntryCounts) -> usize {
    let stats = counts_text(counts).map_or(0, |(a, r)| a.len() + r.len() + 2);
    // code + gaps + name + stats + folder
    2 + 2 + virtual_list::columns(&path.display().to_string()) + stats + 2
}

/// One syntax-highlighted line (tabs shown as four spaces), not wrapped.
pub fn highlighted_line<'a, M: 'a>(
    t: &'a Tokens,
    spans: &'a crate::highlight::Spans,
) -> Element<'a, M> {
    let last = spans.len().saturating_sub(1);
    iced::widget::rich_text(
        spans
            .iter()
            .enumerate()
            .map(|(i, (tone, s))| {
                let s = if i == last {
                    format::code_line(s)
                } else {
                    format::code_text(s)
                };
                iced::widget::span::<(), iced::Font>(s).color(t.tone(*tone))
            })
            .collect::<Vec<_>>(),
    )
    .size(t.font_size)
    .wrapping(iced::widget::text::Wrapping::None)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_label_uses_last_path_component() {
        assert_eq!(file_name_label(std::path::Path::new("lib.rs")), "lib.rs");
        assert_eq!(
            file_name_label(std::path::Path::new("src/lib.rs")),
            "lib.rs"
        );
    }

    #[test]
    fn file_name_label_falls_back_to_path_without_last_component() {
        assert_eq!(file_name_label(std::path::Path::new("/")), "/");
    }

    #[test]
    fn entry_counts_text() {
        let c = |a, r| {
            Some(Some(gf_git::LineCounts {
                added: a,
                removed: r,
            }))
        };
        assert_eq!(counts_text(c(3, 1)), Some(("+3".into(), "-1".into())));
        assert_eq!(counts_text(Some(None)), Some(("bin".into(), String::new())));
        assert_eq!(counts_text(None), None);
    }

    #[test]
    fn entry_width_counts_name_stats_and_folder() {
        let p = std::path::Path::new("src/app/main.rs");
        assert!(
            file_entry_columns(
                p,
                Some(Some(gf_git::LineCounts {
                    added: 12,
                    removed: 3
                }))
            ) > file_entry_columns(p, None)
        );
        // Hangul file names are double width.
        assert!(
            file_entry_columns(std::path::Path::new("문서.md"), None)
                > file_entry_columns(std::path::Path::new("ab.md"), None)
        );
    }
}
