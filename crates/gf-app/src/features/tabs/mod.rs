//! Tab bar: one tab per open repository, tmux-style. Tabs are switched by
//! click or ⌘1–9 and closed by `×` or ⌘W.

use iced::widget::{Row, button, container, row, space};
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

pub fn view<'a>(t: &'a Tokens, tabs: &'a [RepoTab], active: usize) -> Element<'a, Message> {
    let mut bar = Row::new().spacing(1).align_y(iced::Alignment::Center);
    for (i, tab) in tabs.iter().enumerate() {
        let is_active = i == active;
        let branch = tab
            .head_label()
            .map(|b| format!(" ⎇ {b}"))
            .unwrap_or_default();
        let shortcut = if i < 9 {
            format!("{}:", i + 1)
        } else {
            String::new()
        };
        let label = button(widgets::mono(
            t,
            format!("{shortcut}{}{branch}", tab.name()),
        ))
        .padding(Padding::from([2, 8]))
        .style(style::tab(t, is_active))
        .on_press(Message::Select(i));
        let close = button(widgets::mono(t, "×"))
            .padding(Padding::from([2, 6]))
            .style(style::tab(t, is_active))
            .on_press(Message::Close(i));

        let path = tab.repo.cwd();
        let hint = if i < 9 {
            format!(
                "{} · ⌘{} to switch · ⌘W to close",
                format::tilde(path),
                i + 1
            )
        } else {
            format::tilde(path)
        };
        bar = bar.push(widgets::tip(t, row![label, close], hint));
    }
    bar = bar
        .push(widgets::text_button(
            t,
            "+ open",
            Some("⌘O"),
            Some(Message::Open),
        ))
        .push(space().width(Length::Fill));

    container(bar)
        .padding(Padding::from([2, 4]))
        .width(Length::Fill)
        .style(style::bar(t))
        .into()
}
