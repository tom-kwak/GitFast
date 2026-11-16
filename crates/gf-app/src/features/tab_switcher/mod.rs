//! Switching tabs when there are many: tabs in most-recently-used order,
//! the way ⌘⇥ / ⌃⇥ switch apps and editor tabs (VS Code, JetBrains,
//! Sublime Merge):
//!
//! ```text
//! ┌─ SWITCH TAB ───────────── hold ⌃ · ⇥ next · ⇧⇥ back · esc cancel ┐
//! │   1  gf                  ⎇ main            ~/claudework/gf       │
//! │ ▸ 4  order-svc           ⎇ feature/x       ~/work/order-svc      │
//! │   2  billing-svc         ⎇ main            ~/work/billing-svc    │
//! └────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! - A quick ⌃⇥ goes back to the previous tab; ⌃⇥⇥ to the one before.
//! - While ⌃ is held each ⇥ moves down the list (⇧⇥ up, ↑/↓ too);
//!   letting go of ⌃ (or ⏎, or a click) switches, Esc cancels.

use gf_core::model::RepoId;
use iced::widget::{Column, button, column, container, row, space};
use iced::{Background, Border, Element, Length, Padding};

use crate::repo_tab::RepoTab;
use crate::theme::{Tokens, style};
use crate::widgets::{colored, dim, format, mono};

/// Tabs by when they were last active, most recent first.
#[derive(Debug, Clone, Default)]
pub struct Mru(Vec<RepoId>);

impl Mru {
    /// `id` became the active tab.
    pub fn touch(&mut self, id: RepoId) {
        self.0.retain(|i| *i != id);
        self.0.insert(0, id);
    }

    /// Indices of `tabs` (by id), most recently used first; tabs never
    /// active yet come last in tab order.
    pub fn order(&self, tabs: &[RepoId]) -> Vec<usize> {
        let mut order: Vec<usize> = self
            .0
            .iter()
            .filter_map(|id| tabs.iter().position(|t| t == id))
            .collect();
        for i in 0..tabs.len() {
            if !order.contains(&i) {
                order.push(i);
            }
        }
        order
    }
}

/// Position in the MRU list after one more ⇥ (`back`: ⇧⇥). The first
/// press lands on the previous tab, one below the current one.
pub fn step(pos: Option<usize>, n: usize, back: bool) -> usize {
    if n == 0 {
        return 0;
    }
    let pos = pos.unwrap_or(0);
    if back {
        (pos + n - 1) % n
    } else {
        (pos + 1) % n
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// A row was clicked: switch to that tab (index into the tabs).
    Pick(usize),
}

/// The switcher: `order` holds tab indices (MRU), `pos` the highlighted
/// one.
pub fn view<'a>(
    t: &'a Tokens,
    tabs: &'a [RepoTab],
    order: &[usize],
    pos: usize,
) -> Element<'a, Message> {
    let accent = t.accent();
    let mut list = Column::new().spacing(1);
    for (k, &i) in order.iter().enumerate() {
        let Some(tab) = tabs.get(i) else { continue };
        let selected = k == pos;
        let line = row![
            colored(t, if selected { "▸" } else { " " }, accent),
            dim(t, format!("{:>2}", i + 1)).width(Length::Fixed(t.cols(3))),
            colored(t, format::truncate(&tab.name(), 26), t.branch())
                .width(Length::Fixed(t.cols(27))),
            colored(
                t,
                tab.head_label()
                    .map(|h| format!("⎇ {}", format::truncate(&h, 22)))
                    .unwrap_or_default(),
                t.head(),
            )
            .width(Length::Fixed(t.cols(25))),
            dim(t, format::tilde(tab.repo.cwd()))
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        list = list.push(
            button(line)
                .width(Length::Fill)
                .padding(Padding::from([2, 6]))
                .style(style::row_button(t, selected))
                .on_press(Message::Pick(i)),
        );
    }
    let header = row![
        colored(t, "┌─ SWITCH TAB ", accent),
        dim(t, format!("{} open", tabs.len())),
        space().width(Length::Fill),
        dim(
            t,
            "hold ⌃ · ⇥ next · ⇧⇥ back · let go to switch · esc cancel"
        ),
    ]
    .spacing(6);
    container(column![header, list, mono(t, "")].spacing(6))
        .padding(Padding::from([10, 14]))
        .width(Length::Fixed(t.cols(110)))
        .style(move |_| container::Style {
            text_color: Some(t.fg),
            background: Some(Background::Color(t.surface)),
            border: Border {
                color: accent,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recently_used_comes_first_then_the_rest_in_tab_order() {
        let (a, b, c, d) = (RepoId::new(), RepoId::new(), RepoId::new(), RepoId::new());
        let mut mru = Mru::default();
        mru.touch(a);
        mru.touch(c);
        mru.touch(a);
        assert_eq!(mru.order(&[a, b, c, d]), [0, 2, 1, 3]);
        // A closed tab drops out.
        assert_eq!(mru.order(&[b, c]), [1, 0]);
    }

    #[test]
    fn first_press_goes_to_the_previous_tab_and_wraps() {
        assert_eq!(step(None, 3, false), 1);
        assert_eq!(step(Some(1), 3, false), 2);
        assert_eq!(step(Some(2), 3, false), 0);
        assert_eq!(step(None, 3, true), 2);
        assert_eq!(step(Some(0), 3, true), 2);
        assert_eq!(step(None, 0, false), 0);
    }
}
