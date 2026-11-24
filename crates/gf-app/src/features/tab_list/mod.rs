//! The tab dropdown: every open tab in one list under `▾ N tabs` at the
//! right of the tab bar (⌘P), for picking among many tabs without
//! hunting along the strip.
//!
//! ```text
//!                                   ┌────────────────────────────────────────┐
//!                                   │ ❯ ord                                   │
//!                                   │ ▸ 4  order-svc   ⎇ feature/x  ~/w/order × │
//!                                   │   7  ord-batch   ⎇ main       ~/w/ord-b × │
//!                                   └────────────────────────────────────────┘
//! ```
//!
//! - Typing filters (fuzzy on the name, then the path).
//! - ↑/↓, PgUp/PgDn move, ⏎ or a click switches, ⌘W or `×` closes that
//!   tab, Esc clears the filter then closes, a click outside closes.

use iced::widget::{Column, Row, button, container, mouse_area, row, stack, text_input};
use iced::{Color, Element, Length, Padding, mouse};

use crate::features::repo_picker::elide_path;
use crate::repo_tab::RepoTab;
use crate::theme::{Tokens, style};
use crate::widgets::fuzzy::{self, fuzzy};
use crate::widgets::{self, colored, dim, format};

pub const FILTER_ID: &str = "gf-tab-list-filter";

/// Rows shown at once; more scroll.
const ROWS: usize = 14;

/// What the list matches a tab on.
#[derive(Debug, Clone)]
pub struct Key {
    pub name: String,
    pub path: String,
}

impl Key {
    pub fn of(tab: &RepoTab) -> Self {
        Self {
            name: tab.name(),
            path: format::tilde(tab.repo.cwd()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Filter(String),
    /// Switch to this tab (index into the tabs).
    Pick(usize),
    /// Close this tab.
    CloseTab(usize),
    Scroll(mouse::ScrollDelta),
    Dismiss,
}

/// A row: the tab index and the matched characters of its name.
#[derive(Debug, Clone, PartialEq)]
struct Hit {
    tab: usize,
    marks: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    filter: String,
    hits: Vec<Hit>,
    /// Position in `hits`.
    selected: usize,
    /// First row shown.
    first: usize,
}

impl State {
    /// Opened with `active` highlighted.
    pub fn new(keys: &[Key], active: usize) -> Self {
        let mut s = Self {
            hits: matches(keys, ""),
            ..Self::default()
        };
        s.selected = s.hits.iter().position(|h| h.tab == active).unwrap_or(0);
        s.reveal();
        s
    }

    /// Re-runs the filter after tabs were closed: the highlight stays on
    /// the same row (the next tab moves up into it).
    pub fn refilter(&mut self, keys: &[Key]) {
        self.hits = matches(keys, &self.filter);
        self.selected = self.selected.min(self.hits.len().saturating_sub(1));
        self.reveal();
    }

    pub fn set_filter(&mut self, filter: String, keys: &[Key]) {
        self.filter = filter;
        self.hits = matches(keys, &self.filter);
        self.selected = 0;
        self.first = 0;
    }

    /// Esc: clears the filter first; `true` when the list should close.
    pub fn escape(&mut self, keys: &[Key]) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        self.set_filter(String::new(), keys);
        false
    }

    pub fn move_by(&mut self, by: isize) {
        let n = self.hits.len();
        if n == 0 {
            return;
        }
        let last = n - 1;
        self.selected = self.selected.saturating_add_signed(by).min(last);
        self.reveal();
    }

    pub fn page() -> isize {
        isize::try_from(ROWS - 1).unwrap_or(1)
    }

    /// The highlighted tab.
    pub fn selected(&self) -> Option<usize> {
        self.hits.get(self.selected).map(|h| h.tab)
    }

    fn reveal(&mut self) {
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + ROWS {
            self.first = self.selected + 1 - ROWS;
        }
        self.first = self.first.min(self.hits.len().saturating_sub(ROWS));
    }

    fn scroll(&mut self, delta: mouse::ScrollDelta) {
        let lines = match delta {
            mouse::ScrollDelta::Lines { y, .. } => y,
            mouse::ScrollDelta::Pixels { y, .. } => y / 20.0,
        };
        #[allow(clippy::cast_possible_truncation)]
        let by = -(lines * 3.0).round() as isize;
        let max = self.hits.len().saturating_sub(ROWS);
        self.first = self.first.saturating_add_signed(by).min(max);
        // The highlight stays on screen.
        self.selected = self
            .selected
            .clamp(self.first, (self.first + ROWS).saturating_sub(1));
    }

    /// Handles a message from the view; scrolling is handled here, the
    /// rest is for the app.
    pub fn update(&mut self, message: &Message) {
        if let Message::Scroll(d) = message {
            self.scroll(*d);
        }
    }

    /// The dropdown, hanging from the right end of the tab bar, over a
    /// transparent layer that closes it when clicked.
    pub fn view<'a>(
        &'a self,
        t: &'a Tokens,
        tabs: &'a [RepoTab],
        active: usize,
    ) -> Element<'a, Message> {
        let accent = t.accent();
        let filter = row![
            colored(t, "❯", accent),
            text_input("filter tabs", &self.filter)
                .id(FILTER_ID)
                .on_input(Message::Filter)
                .size(t.font_size)
                .padding(3)
                .style(style::field_input(t)),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        let mut list = Column::new().spacing(1);
        for (k, hit) in self.hits.iter().enumerate().skip(self.first).take(ROWS) {
            let Some(tab) = tabs.get(hit.tab) else {
                continue;
            };
            list = list.push(self.row(t, tab, hit, k == self.selected, hit.tab == active));
        }
        let list: Element<'a, Message> = if self.hits.is_empty() {
            widgets::empty_hint(t, "no tab matches")
        } else {
            mouse_area(list).on_scroll(Message::Scroll).into()
        };

        let n = self.hits.len();
        let range = if n > ROWS {
            format!(
                "{}–{} of {n} · ",
                self.first + 1,
                (self.first + ROWS).min(n)
            )
        } else {
            String::new()
        };
        let footer = dim(t, format!("{range}↑↓ move · ⏎ switch · ⌘W close tab · esc"));

        let panel = container(column_of(vec![filter.into(), list, footer.into()]))
            .padding(Padding::from([6, 8]))
            .width(Length::Fill)
            .max_width(t.cols(96))
            .style(style::tooltip(t));
        // Just under the tab bar, its right edge under the `▾ N tabs`
        // button (left of `[ + open ⌘O ]` at the far right).
        let placed = container(panel)
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .padding(Padding {
                top: (t.font_size * 2.0).round(),
                right: t.cols(13) + 12.0,
                bottom: 0.0,
                left: 0.0,
            });
        let backdrop = mouse_area(
            container(iced::widget::space())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Color::TRANSPARENT.into()),
                    ..container::Style::default()
                }),
        )
        .on_press(Message::Dismiss)
        .on_right_press(Message::Dismiss);
        stack![backdrop, placed].into()
    }

    fn row<'a>(
        &self,
        t: &'a Tokens,
        tab: &'a RepoTab,
        hit: &Hit,
        selected: bool,
        active: bool,
    ) -> Element<'a, Message> {
        let mut name = Row::new();
        let shown = format::truncate(&tab.name(), 26);
        for (i, ch) in shown.chars().enumerate() {
            let color = if hit.marks.contains(&i) {
                t.accent()
            } else {
                t.branch()
            };
            name = name.push(colored(t, ch.to_string(), color));
        }
        let number = if hit.tab < 9 {
            format!("⌘{}", hit.tab + 1)
        } else {
            format!("{:>2}", hit.tab + 1)
        };
        let line = row![
            colored(t, if selected { "▸" } else { " " }, t.accent()),
            colored(t, if active { "●" } else { " " }, t.accent()),
            dim(t, number).width(Length::Fixed(t.cols(3))),
            container(name).width(Length::Fixed(t.cols(27))),
            colored(
                t,
                tab.head_label()
                    .map(|h| format!("⎇ {}", format::truncate(&h, 20)))
                    .unwrap_or_default(),
                t.head(),
            )
            .width(Length::Fixed(t.cols(23)))
            .wrapping(iced::widget::text::Wrapping::None),
            dim(t, elide_path(&format::tilde(tab.repo.cwd()), 34))
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        let pick = button(line)
            .width(Length::Fill)
            .padding(Padding::from([2, 4]))
            .style(style::row_button(t, selected))
            .on_press(Message::Pick(hit.tab));
        let close = widgets::tip(
            t,
            button(widgets::mono(t, "×"))
                .padding(Padding::from([2, 6]))
                .style(style::row_button(t, selected))
                .on_press(Message::CloseTab(hit.tab)),
            "close this tab".into(),
        );
        row![pick, close].spacing(1).into()
    }
}

fn column_of(items: Vec<Element<'_, Message>>) -> Column<'_, Message> {
    Column::with_children(items).spacing(6)
}

/// Tabs matching `filter`: all in tab order when empty; otherwise name
/// matches first (best first), then tabs whose path matches.
fn matches(keys: &[Key], filter: &str) -> Vec<Hit> {
    if filter.trim().is_empty() {
        return (0..keys.len())
            .map(|tab| Hit {
                tab,
                marks: Vec::new(),
            })
            .collect();
    }
    let mut scored: Vec<(i32, Hit)> = keys
        .iter()
        .enumerate()
        .filter_map(|(tab, k)| {
            if let Some((score, marks)) = fuzzy(filter, &k.name) {
                return Some((score + fuzzy::RANK, Hit { tab, marks }));
            }
            fuzzy(filter, &k.path).map(|(score, _)| {
                (
                    score,
                    Hit {
                        tab,
                        marks: Vec::new(),
                    },
                )
            })
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(_, h)| h).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(names: &[&str]) -> Vec<Key> {
        names
            .iter()
            .map(|n| Key {
                name: (*n).into(),
                path: format!("~/work/{n}"),
            })
            .collect()
    }

    fn tabs_of(s: &State) -> Vec<usize> {
        s.hits.iter().map(|h| h.tab).collect()
    }

    #[test]
    fn opens_on_the_active_tab_in_tab_order() {
        let k = keys(&["gf", "order-svc", "billing-svc"]);
        let s = State::new(&k, 2);
        assert_eq!(tabs_of(&s), [0, 1, 2]);
        assert_eq!(s.selected(), Some(2));
    }

    #[test]
    fn filter_matches_names_before_paths() {
        let k = vec![
            Key {
                name: "api".into(),
                path: "~/svc/order/api".into(),
            },
            Key {
                name: "order-svc".into(),
                path: "~/work/order-svc".into(),
            },
            Key {
                name: "gf".into(),
                path: "~/claudework/gf".into(),
            },
        ];
        let mut s = State::new(&k, 0);
        s.set_filter("order".into(), &k);
        assert_eq!(tabs_of(&s), [1, 0], "name match, then path match");
        assert_eq!(s.selected(), Some(1));
        assert!(!s.escape(&k), "esc clears the filter first");
        assert_eq!(tabs_of(&s).len(), 3);
        assert!(s.escape(&k));
    }

    #[test]
    fn moving_scrolls_the_window_and_stays_in_range() {
        let names: Vec<String> = (0..30).map(|i| format!("svc-{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let k = keys(&refs);
        let mut s = State::new(&k, 0);
        s.move_by(20);
        assert_eq!(s.selected(), Some(20));
        assert_eq!(s.first, 20 + 1 - ROWS);
        s.move_by(100);
        assert_eq!(s.selected(), Some(29));
        s.move_by(-100);
        assert_eq!((s.selected(), s.first), (Some(0), 0));
    }

    #[test]
    fn closing_a_tab_keeps_the_highlight_on_the_same_tab() {
        let k = keys(&["a", "b", "c"]);
        let mut s = State::new(&k, 2);
        // Tab 0 closed: what was tab 2 is now tab 1.
        let k = keys(&["b", "c"]);
        s.refilter(&k);
        assert_eq!(s.selected(), Some(1));
    }
}
