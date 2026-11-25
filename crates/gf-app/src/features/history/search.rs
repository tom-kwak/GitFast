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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Files,
    Commits,
}

impl Mode {
    pub fn other(self) -> Self {
        match self {
            Self::Files => Self::Commits,
            Self::Commits => Self::Files,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    QueryChanged(String),
    Mode(Mode),
    Retry,
    Composing(bool),
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
    pub composing: bool,
}

impl State {
    pub fn is_ready(&self) -> bool {
        self.results_ready
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn reset_results(&mut self) {
        let query = std::mem::take(&mut self.query);
        *self = Self {
            query,
            ..Self::default()
        };
    }

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
            Message::Composing(active) => self.composing = active,
            Message::Mode(_) | Message::Retry => {}
        }
    }

    pub fn view<'a>(
        &'a self,
        t: &'a Tokens,
        props: View<'a>,
        render: impl Fn(usize, bool) -> Element<'a, Message> + 'a,
    ) -> Element<'a, Message> {
        let View {
            mode,
            scope,
            status,
            error,
            truncated,
            height,
            locating,
        } = props;
        let mode_button = |candidate, label| {
            let color = t.accent();
            button(colored(t, format!("[ {label} ]"), color))
                .padding(Padding::from([1, 2]))
                .style(style::text_button(t, color))
                .on_press_maybe((mode != candidate).then_some(Message::Mode(candidate)))
        };
        let controls = row![
            mode_button(
                Mode::Files,
                if mode == Mode::Files {
                    "■ Files"
                } else {
                    "□ Files"
                }
            ),
            mode_button(
                Mode::Commits,
                if mode == Mode::Commits {
                    "■ Commits"
                } else {
                    "□ Commits"
                }
            ),
            dim(t, scope)
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::Word),
            widgets::icon_button(t, "×", t.dim, "close search", Message::Dismiss),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        let input = row![
            colored(t, "❯", t.accent()),
            text_input(
                if mode == Mode::Files {
                    "file name or path…"
                } else {
                    "commit message or hash…"
                },
                &self.query,
            )
            .id(INPUT_ID)
            .on_input(Message::QueryChanged)
            .size(t.font_size)
            .padding(4)
            .style(style::field_input(t)),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        let results: Element<'a, Message> = if let Some(error) = error {
            column![
                widgets::empty_hint(t, error),
                widgets::text_button(t, "Retry", None, Some(Message::Retry)),
            ]
            .into()
        } else if !self.shown_once || self.result_count == 0 {
            widgets::empty_hint(t, status.as_deref().unwrap_or("Searching…").to_owned())
        } else {
            virtual_list(
                LIST_ID,
                self.result_count,
                height,
                if mode == Mode::Files { t.cols(96) } else { 0.0 },
                self.scroll,
                move |index| {
                    let selected = self.results_ready && index == self.selected;
                    button(
                        row![
                            colored(t, if selected { "›" } else { " " }, t.accent()),
                            render(index, self.results_ready && !locating),
                        ]
                        .spacing(4)
                        .align_y(iced::Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding(Padding::from([2, 4]))
                    .style(style::row_button(t, selected))
                    .on_press_maybe(
                        (self.results_ready && !locating).then_some(Message::Activate(index)),
                    )
                    .into()
                },
                Message::Scrolled,
            )
        };
        let summary = if truncated {
            format!(
                "Showing first {} commits · narrow your search",
                gf_git::COMMIT_SEARCH_LIMIT
            )
        } else {
            let noun = if mode == Mode::Files {
                "file"
            } else {
                "commit"
            };
            widgets::format::count(self.result_count as u64, noun)
        };
        let action = if mode == Mode::Files {
            "show file history"
        } else {
            "show commit"
        };
        let footer = if locating {
            "Finding commit in history… · esc close".into()
        } else if !self.results_ready && self.shown_once && error.is_none() {
            "Searching… · ⇥ switch mode · esc close".into()
        } else if self.result_count == 0 || error.is_some() {
            "⇥ switch mode · esc close".into()
        } else {
            format!("{summary} · ⇥ switch mode · ↑↓ move · ⏎ {action} · esc close")
        };
        column![
            controls,
            input,
            results,
            dim(t, footer).wrapping(iced::widget::text::Wrapping::Word)
        ]
        .spacing(4)
        .height(Length::Fill)
        .into()
    }
}

pub struct View<'a> {
    pub mode: Mode,
    pub scope: String,
    pub status: Option<String>,
    pub error: Option<&'a str>,
    pub truncated: bool,
    pub height: f32,
    pub locating: bool,
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
