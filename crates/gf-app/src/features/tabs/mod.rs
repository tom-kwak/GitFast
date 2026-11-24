//! Tab bar: one tab per open repository, tmux-style. Tabs are switched by
//! click, ⌘1–9, ⌘⇧[ / ⌘⇧], ⌃⇥ (most recently used, see `tab_switcher`)
//! or the `▾ N tabs` dropdown (⌘P, see `tab_list`), and closed by `×` or
//! ⌘W. Many tabs scroll sideways and draw compact. A tab is moved by
//! dragging it like a browser tab (it follows the cursor, the others make
//! room; see `widgets::reorder`) or with ⌘⌥⇧[ / ⌘⌥⇧].

use iced::widget::{button, container, row, scrollable};
use iced::{Element, Length, Padding};

use crate::repo_tab::RepoTab;
use crate::theme::{Tokens, style};
use crate::widgets::reorder::Reorder;
use crate::widgets::{self, format};

#[derive(Debug, Clone)]
pub enum Message {
    Select(usize),
    Close(usize),
    Open,
    Settings,
    /// `▾ N tabs`: open (or close) the tab dropdown.
    List,
    /// Tab `from` was dragged to position `to`.
    Moved(usize, usize),
}

/// Where the tab at `i` is after the tab at `from` moved to `to`.
pub fn index_after_move(i: usize, from: usize, to: usize) -> usize {
    if i == from {
        to
    } else if from < i && i <= to {
        i - 1
    } else if to <= i && i < from {
        i + 1
    } else {
        i
    }
}

/// The tooltip of tab `i`: where it is, its branch and its keys.
fn hint(tab: &RepoTab, i: usize) -> String {
    let mut hint = format::tilde(tab.repo.cwd());
    if let Some(b) = tab.head_label() {
        hint = format!("{hint} ⎇ {b}");
    }
    if i < 9 {
        hint = format!("{hint} · ⌘{} to switch · ⌘W to close", i + 1);
    }
    format!("{hint} · drag (or ⌘⌥⇧[ ⌘⌥⇧]) to move")
}

/// Id of the scrollable tab strip (scrolled to keep the active tab in view).
pub const BAR_ID: &str = "gf-tab-bar";

/// Tabs from this many on are drawn compact (inactive names shortened).
const COMPACT_FROM: usize = 5;

pub fn view<'a>(
    t: &'a Tokens,
    tabs: &'a [RepoTab],
    active: usize,
    list_open: bool,
) -> Element<'a, Message> {
    // Tabs show the repository name only (the branch is in the toolbar,
    // the status line and the tooltip); with many tabs the inactive names
    // are shortened so a dozen services still fit on one line.
    let compact = tabs.len() >= COMPACT_FROM;
    let mut items = Vec::with_capacity(tabs.len());
    for (i, tab) in tabs.iter().enumerate() {
        let is_active = i == active;
        let shortcut = if i < 9 {
            format!("{}:", i + 1)
        } else {
            String::new()
        };
        let name = if compact && !is_active {
            format::truncate(&tab.name(), 16)
        } else {
            tab.name()
        };
        let label = button(widgets::mono(t, format!("{shortcut}{name}")))
            .padding(Padding::from([2, 8]))
            .style(style::tab(t, is_active))
            .on_press(Message::Select(i));
        let close = button(widgets::mono(t, "×"))
            .padding(Padding::from([2, 6]))
            .style(style::tab(t, is_active))
            .on_press(Message::Close(i));
        items.push(widgets::tip(t, row![label, close], hint(tab, i)));
    }
    // A picked-up tab is shown at once, like a browser tab.
    let bar = Reorder::new(items)
        .spacing(1.0)
        .on_pick(Message::Select)
        .on_drop(Message::Moved);
    let strip = scrollable(bar)
        .id(BAR_ID)
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(2).scroller_width(2),
        ))
        .width(Length::Fill);
    // The strip takes the room left; the tab dropdown and `+ open` (at the
    // far right) stay visible.
    let mut line = row![strip].spacing(4).align_y(iced::Alignment::Center);
    if tabs.len() > 1 {
        line = line.push(widgets::tip(
            t,
            button(widgets::mono(t, format!("▾ {} tabs ⌘P", tabs.len())))
                .padding(Padding::from([2, 8]))
                .style(style::tab(t, list_open))
                .on_press(Message::List),
            "all tabs in a list, type to filter · ⌃⇥ previous tab · ⌘⇧[ ⌘⇧] left / right · ⌘1–9"
                .into(),
        ));
    }
    if !tabs.is_empty() {
        line = line.push(widgets::text_button(
            t,
            "+ open",
            Some("⌘O"),
            Some(Message::Open),
        ));
    }
    line = line.push(widgets::tip(
        t,
        widgets::text_button(t, "Settings", Some("⌘,"), Some(Message::Settings)),
        "Open app settings (⌘,)".into(),
    ));

    container(line)
        .padding(Padding::from([2, 4]))
        .width(Length::Fill)
        .style(style::bar(t))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_tabs_shift_to_make_room_for_the_moved_one() {
        // a b c d, b → after d: a c d b
        let after: Vec<_> = (0..4).map(|i| index_after_move(i, 1, 3)).collect();
        assert_eq!(after, [0, 3, 1, 2]);
        // a b c d, d → before a: d a b c
        let after: Vec<_> = (0..4).map(|i| index_after_move(i, 3, 0)).collect();
        assert_eq!(after, [1, 2, 3, 0]);
        assert_eq!(index_after_move(2, 1, 1), 2);
    }
}
