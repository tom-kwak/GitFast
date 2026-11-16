//! Tab bar: one tab per open repository, tmux-style. Tabs are switched by
//! click, ⌘1–9, ⌘⇧[ / ⌘⇧] or ⌃⇥ (most recently used, see `tab_switcher`)
//! and closed by `×` or ⌘W. Many tabs scroll sideways and draw compact.

use iced::widget::{Row, button, container, row, scrollable};
use iced::{Element, Length, Padding};

use crate::repo_tab::RepoTab;
use crate::theme::{Tokens, style};
use crate::widgets::{self, format};

#[derive(Debug, Clone)]
pub enum Message {
    Select(usize),
    Close(usize),
    Open,
}

/// Id of the scrollable tab strip (scrolled to keep the active tab in view).
pub const BAR_ID: &str = "gf-tab-bar";

/// Tabs from this many on are drawn compact (name only, shortened).
const COMPACT_FROM: usize = 5;

pub fn view<'a>(t: &'a Tokens, tabs: &'a [RepoTab], active: usize) -> Element<'a, Message> {
    // With many tabs only the active one shows its branch, and names are
    // shortened, so a dozen services still fit on one line.
    let compact = tabs.len() >= COMPACT_FROM;
    let mut bar = Row::new().spacing(1).align_y(iced::Alignment::Center);
    for (i, tab) in tabs.iter().enumerate() {
        let is_active = i == active;
        let branch = match tab.head_label() {
            Some(b) if is_active || !compact => format!(" ⎇ {}", format::truncate(&b, 28)),
            _ => String::new(),
        };
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
        let label = button(widgets::mono(t, format!("{shortcut}{name}{branch}")))
            .padding(Padding::from([2, 8]))
            .style(style::tab(t, is_active))
            .on_press(Message::Select(i));
        let close = button(widgets::mono(t, "×"))
            .padding(Padding::from([2, 6]))
            .style(style::tab(t, is_active))
            .on_press(Message::Close(i));

        let path = tab.repo.cwd();
        let mut hint = format::tilde(path);
        if let Some(b) = tab.head_label() {
            hint = format!("{hint} ⎇ {b}");
        }
        if i < 9 {
            hint = format!("{hint} · ⌘{} to switch · ⌘W to close", i + 1);
        }
        bar = bar.push(widgets::tip(t, row![label, close], hint));
    }
    let strip = scrollable(bar)
        .id(BAR_ID)
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(2).scroller_width(2),
        ))
        .width(Length::Fill);
    // The strip takes the room left; `+ open` and the hint stay visible.
    let mut line = row![
        strip,
        widgets::text_button(t, "+ open", Some("⌘O"), Some(Message::Open))
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);
    if tabs.len() > 1 {
        line = line.push(widgets::tip(
            t,
            widgets::dim(t, format!("{} tabs · ⌃⇥ switch", tabs.len())),
            "⌃⇥ previous tab (hold ⌃ and press ⇥ again to go further back) · ⌘⇧[ ⌘⇧] left / right · ⌘1–9".into(),
        ));
    }

    container(line)
        .padding(Padding::from([2, 4]))
        .width(Length::Fill)
        .style(style::bar(t))
        .into()
}
