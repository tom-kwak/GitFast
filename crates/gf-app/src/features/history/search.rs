//! The search box and result list shown in the History panel.
//!
//! It knows only a result count: the owner turns a row number into
//! whatever it found (a path today, maybe a commit later), and renders the
//! rows. Moving, activating and closing are the same for any result kind.

use iced::widget::{button, column, row, text_input};
use iced::{Element, Length, Padding, Task};

use crate::nav::{self, Key};
use crate::theme::{Tokens, style};
use crate::widgets::virtual_list::{ScrollPos, virtual_list};
use crate::widgets::{self, colored, dim};

pub const INPUT_ID: &str = "gf-history-file-search";
pub const LIST_ID: &str = "gf-history-file-results";

#[derive(Debug, Clone)]
pub enum Message {
    QueryChanged(String),
    Activate(usize),
    Dismiss,
    Scrolled(ScrollPos),
}

#[derive(Debug, Default)]
pub struct State {
    query: String,
    selected: usize,
    result_count: usize,
    /// The results belong to the current query (rows can be picked).
    results_ready: bool,
    /// Some results were shown since the search opened. While a newer query
    /// runs they stay on screen (not pickable), so typing does not flash.
    shown_once: bool,
    scroll: ScrollPos,
}

impl State {
    pub fn query_changed(&mut self, query: String) {
        self.query = query;
        self.results_ready = false;
    }

    pub fn dismiss(&mut self) {
        *self = Self::default();
    }

    pub fn results(&mut self, count: usize) {
        self.selected = 0;
        self.result_count = count;
        self.results_ready = true;
        self.shown_once = true;
        self.scroll = ScrollPos::default();
    }

    pub fn move_to(&mut self, key: Key, row_height: f32) -> bool {
        if !self.results_ready || self.result_count == 0 {
            return false;
        }
        let Some(next) = nav::step(
            Some(self.selected),
            self.result_count,
            key,
            nav::page(self.scroll, row_height),
        ) else {
            return false;
        };
        self.selected = next;
        true
    }

    pub fn activate(&mut self, row: usize) -> Option<usize> {
        if !self.results_ready || row >= self.result_count {
            return None;
        }
        self.selected = row;
        Some(row)
    }

    pub fn selected(&self) -> Option<usize> {
        (self.results_ready && self.result_count > 0).then_some(self.selected)
    }

    #[cfg(test)]
    pub fn result_count(&self) -> usize {
        self.result_count
    }

    /// Back to the first row, once new results replace the old ones.
    pub fn scroll_to_top<M: Send + 'static>() -> Task<M> {
        nav::scroll_y(LIST_ID, 0.0)
    }

    pub fn scroll_to_selected<M: Send + 'static>(&self, row_height: f32) -> Task<M> {
        self.selected().map_or_else(Task::none, |selected| {
            nav::reveal(LIST_ID, self.scroll, selected, row_height)
        })
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::QueryChanged(query) => self.query_changed(query),
            Message::Activate(row) => {
                self.activate(row);
            }
            Message::Scrolled(pos) => self.scroll = pos,
            Message::Dismiss => self.dismiss(),
        }
    }

    pub fn view<'a>(
        &'a self,
        t: &'a Tokens,
        hint: &'a str,
        result_label: &str,
        status: Option<&'a str>,
        render: impl Fn(usize, bool) -> Element<'a, Message> + 'a,
    ) -> Element<'a, Message> {
        let input = row![
            colored(t, "❯", t.accent()),
            text_input(hint, &self.query)
                .id(INPUT_ID)
                .on_input(Message::QueryChanged)
                .size(t.font_size)
                .padding(4)
                .style(style::field_input(t)),
            widgets::icon_button(t, "×", t.dim, "close search", Message::Dismiss),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        // Until the first results, say what is going on; after that keep the
        // previous rows on screen while the next ones are computed.
        let results: Element<'a, Message> = if !self.shown_once {
            widgets::empty_hint(t, status.unwrap_or("Searching…"))
        } else if self.result_count == 0 {
            widgets::empty_hint(t, "no files match")
        } else {
            virtual_list(
                LIST_ID,
                self.result_count,
                row_height(t),
                t.cols(96),
                self.scroll,
                move |index| {
                    let selected = self.results_ready && index == self.selected;
                    button(render(index, selected))
                        .width(Length::Fill)
                        .padding(Padding::from([2, 4]))
                        .style(style::row_button(t, selected))
                        .on_press(Message::Activate(index))
                        .into()
                },
                Message::Scrolled,
            )
        };
        let footer = if self.shown_once {
            dim(
                t,
                format!(
                    "{} {result_label} · ↑↓ move · ⏎ select · esc close",
                    self.result_count,
                ),
            )
        } else {
            dim(t, status.unwrap_or("Searching…"))
        };
        column![input, results, footer]
            .spacing(4)
            .height(Length::Fill)
            .into()
    }
}

pub fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.8).round()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_activation_and_cancel_need_only_a_result_count() {
        let mut state = State::default();
        state.results(4);
        assert!(state.move_to(Key::Down, 20.0));
        assert!(state.move_to(Key::Down, 20.0));
        assert_eq!(state.activate(2), Some(2));
        assert_eq!(state.selected(), Some(2));
        state.update(Message::Dismiss);
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn a_query_and_new_results_reset_the_highlight() {
        let mut state = State::default();
        state.results(5);
        assert_eq!(state.activate(4), Some(4));
        state.query_changed("src".into());
        assert_eq!(state.selected(), None, "old results cannot be activated");
        assert_eq!(state.activate(4), None);
        assert_eq!(state.result_count(), 5, "the old rows stay on screen");
        state.results(2);
        assert_eq!(state.selected(), Some(0));
        assert_eq!(state.result_count(), 2);
    }
}
