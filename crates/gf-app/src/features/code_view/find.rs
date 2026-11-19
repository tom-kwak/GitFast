//! Literal, case-insensitive search over the rows displayed by the file viewer.

use std::ops::Range;
use std::sync::Arc;

use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};
use iced::advanced::widget::{Id, Operation};
use iced::widget::{operation, row, text_input};
use iced::{Element, Length, Rectangle, Task, Vector};
use regex::RegexBuilder;

use super::{
    Code, Document, LIST_ID, Message, Output, Pane, PreviewRow, State, line_width, row_height,
};
use crate::feature::Update;
use crate::theme::{Tokens, style};
use crate::widgets::{self, dim};

pub(crate) const FIELD_ID: &str = "gf-code-find";

#[derive(Debug, Default)]
pub(super) struct Find {
    open: bool,
    query: String,
    generation: u64,
    pending: bool,
    hits: Arc<[Hit]>,
    active: usize,
}

/// A match in display order, with a byte range in the original line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    row: u32,
    pane: Pane,
    start: u32,
    end: u32,
}

impl Hit {
    fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

impl State {
    /// Whether Escape belongs to the search bar.
    pub(crate) fn find_is_open(&self) -> bool {
        self.find.open
    }

    pub(super) fn open_find(&mut self) -> Update<Message, Output> {
        if self.code().is_none() {
            return Update::none();
        }
        let task = if self.find.open {
            Task::none()
        } else {
            self.find.open = true;
            self.search()
        };
        Update::task(Task::batch([
            task,
            operation::focus(FIELD_ID).chain(operation::select_all(FIELD_ID)),
        ]))
    }

    pub(super) fn close_find(&mut self) {
        self.find.open = false;
        self.find.clear_results();
    }

    pub(super) fn find_query(&mut self, query: String) -> Task<Message> {
        self.find.query = query;
        self.search()
    }

    pub(super) fn search(&mut self) -> Task<Message> {
        self.find.clear_results();
        let query = &self.find.query;
        let Some(Ok(document)) = self
            .loaded
            .as_ref()
            .filter(|_| self.find.open && !query.is_empty())
        else {
            return Task::none();
        };
        let document = document.clone();
        let query = query.clone();
        let split = self.split;
        let request = self.request;
        let generation = self.find.generation;
        self.find.pending = true;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || match document.as_ref() {
                    Document::Text(code) => matches(code, split, &query),
                    _ => Arc::default(),
                })
                .await
                .unwrap_or_default()
            },
            move |hits| Message::FindResults(request, generation, hits),
        )
    }

    pub(super) fn find_results(
        &mut self,
        request: u64,
        generation: u64,
        hits: Arc<[Hit]>,
    ) -> Update<Message, Output> {
        if self.request != request || self.find.generation != generation || !self.find.pending {
            return Update::none();
        }
        self.find.pending = false;
        self.find.hits = hits;
        self.find.reveal()
    }

    pub(super) fn next_match(&mut self, back: bool) -> Update<Message, Output> {
        let len = self.find.hits.len();
        if len == 0 {
            return Update::none();
        }
        self.find.active = if back {
            self.find.active.checked_sub(1).unwrap_or(len - 1)
        } else {
            (self.find.active + 1) % len
        };
        self.find.reveal()
    }

    /// Reveal the active match using the target pane's actual viewport dimensions.
    pub(crate) fn reveal_match(&self, t: &Tokens) -> Task<Message> {
        let Some(hit) = self.find.hits.get(self.find.active) else {
            return Task::none();
        };
        let Some(code) = self.code() else {
            return Task::none();
        };
        let row = hit.row as usize;
        let (raw, gutter, list) = if self.split {
            let Some(aligned) = code.comparison.as_ref().and_then(|c| c.rows.get(row)) else {
                return Task::none();
            };
            let index = match hit.pane {
                Pane::Before => aligned.before,
                Pane::After => aligned.after,
            };
            let Some((source, index)) = self.pane_code(hit.pane).zip(index) else {
                return Task::none();
            };
            (source.line(index), 3, hit.pane.list_id())
        } else {
            (
                inline_text(code, row),
                code.max_line_number.to_string().len().max(3) + 3,
                LIST_ID,
            )
        };
        #[allow(clippy::cast_precision_loss)] // The row index becomes a pixel coordinate.
        let y = row as f32 * row_height(t);
        let x = t.cols(gutter + line_width(&raw[..hit.range().start]));
        let end = t.cols(gutter + line_width(&raw[..hit.range().end]));
        iced::advanced::widget::operate(Reveal {
            target: Id::from(list),
            x: x..end,
            y: y..y + row_height(t),
        })
    }

    pub(super) fn find_bar<'a>(&'a self, t: &'a Tokens) -> Element<'a, Message> {
        let count = match (self.find.pending, self.find.hits.len()) {
            (true, _) => "Searching…".into(),
            (false, 0) if self.find.query.is_empty() => String::new(),
            (false, 0) => "0 of 0".into(),
            (false, len) => format!("{} of {len}", self.find.active + 1),
        };
        let can_move = self.find.hits.len() > 1;
        row![
            text_input("Find in file…", &self.find.query)
                .id(FIELD_ID)
                .on_input(Message::FindQuery)
                .size(t.font_size)
                .padding(4)
                .width(Length::Fill)
                .style(style::field_input(t)),
            dim(t, count),
            widgets::icon_button_maybe(
                t,
                "↑",
                t.accent(),
                "Previous match · ⇧Enter / ⌘⇧G",
                can_move.then_some(Message::FindMove { back: true }),
            ),
            widgets::icon_button_maybe(
                t,
                "↓",
                t.accent(),
                "Next match · Enter / ⌘G",
                can_move.then_some(Message::FindMove { back: false }),
            ),
            widgets::icon_button(t, "✕", t.dim, "Close search · Esc", Message::FindClose),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .into()
    }
}

impl Find {
    fn clear_results(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
        self.hits = Arc::default();
        self.active = 0;
    }

    fn reveal(&self) -> Update<Message, Output> {
        if self.hits.is_empty() {
            Update::none()
        } else {
            Update::output(Output::RevealMatch)
        }
    }

    pub(super) fn row_hits(
        &self,
        row: usize,
        pane: Pane,
    ) -> impl Iterator<Item = (Range<usize>, bool)> + '_ {
        let start = self.hits.partition_point(|hit| (hit.row as usize) < row);
        self.hits[start..]
            .iter()
            .take_while(move |hit| hit.row as usize == row)
            .enumerate()
            .filter(move |(_, hit)| hit.pane == pane)
            .map(move |(i, hit)| (hit.range(), start + i == self.active))
    }
}

fn inline_text(code: &Code, row: usize) -> &str {
    match code.preview_rows.as_ref().map(|rows| &rows[row]) {
        Some(PreviewRow::Removed { text, .. }) => text,
        Some(PreviewRow::File(index, _)) => code.line(*index),
        None => code.line(row),
    }
}

fn matches(code: &Code, split: bool, query: &str) -> Arc<[Hit]> {
    let Ok(pattern) = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
    else {
        return Arc::default();
    };
    let mut hits = Vec::new();
    let mut scan = |row, pane, text: &str| {
        hits.extend(pattern.find_iter(text).map(|m| Hit {
            row: u32::try_from(row).expect("preview row fits in u32"),
            pane,
            start: u32::try_from(m.start()).expect("preview column fits in u32"),
            end: u32::try_from(m.end()).expect("preview column fits in u32"),
        }));
    };
    if split && let Some(comparison) = &code.comparison {
        let before = if comparison.deleted {
            Some(code)
        } else {
            comparison.before.as_deref()
        };
        for (row, aligned) in comparison.rows.iter().enumerate() {
            if let Some((before, index)) = before.zip(aligned.before) {
                scan(row, Pane::Before, before.line(index));
            }
            if let Some(index) = aligned.after {
                scan(row, Pane::After, code.line(index));
            }
        }
    } else {
        let len = code
            .preview_rows
            .as_ref()
            .map_or(code.lines.len(), Vec::len);
        for row in 0..len {
            scan(row, Pane::After, inline_text(code, row));
        }
    }
    hits.into()
}

struct Reveal {
    target: Id,
    x: Range<f32>,
    y: Range<f32>,
}

impl Operation<Message> for Reveal {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Message>)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content: Rectangle,
        translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        if id != Some(&self.target) {
            return;
        }
        let margin = crate::widgets::virtual_list::SCROLLBAR;
        let width = bounds.width
            - if content.height > bounds.height {
                margin
            } else {
                0.0
            };
        let height = bounds.height
            - if content.width > bounds.width {
                margin
            } else {
                0.0
            };
        let x = reveal_axis(&self.x, translation.x, width);
        let y = reveal_axis(&self.y, translation.y, height)
            .map(|_| ((self.y.start + self.y.end - height) / 2.0).max(0.0));
        if x.is_some() || y.is_some() {
            state.scroll_to(AbsoluteOffset { x, y });
        }
    }
}

fn reveal_axis(hit: &Range<f32>, offset: f32, size: f32) -> Option<f32> {
    if hit.start < offset {
        Some(hit.start)
    } else if hit.end > offset + size {
        Some((hit.end - size).min(hit.start))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::feature::Feature;

    #[test]
    fn matches_literal_text_case_insensitively_with_original_byte_ranges() {
        let text = "한글 Ä. ä. äX\r\nä.";
        let Document::Text(code) =
            Document::from_bytes(Path::new("notes.txt"), text.as_bytes().to_vec())
        else {
            panic!("expected text");
        };
        let hits = matches(&code, false, "ä.");
        let locations: Vec<_> = hits
            .iter()
            .map(|hit| (hit.row, &code.line(hit.row as usize)[hit.range()]))
            .collect();
        assert_eq!(locations, [(0, "Ä."), (0, "ä."), (1, "ä.")]);
    }

    #[test]
    fn reveals_offscreen_matches_without_moving_visible_matches() {
        assert_eq!(reveal_axis(&(130.0..140.0), 100.0, 50.0), None);
        assert_eq!(reveal_axis(&(80.0..85.0), 100.0, 50.0), Some(80.0));
        assert_eq!(reveal_axis(&(160.0..170.0), 100.0, 50.0), Some(120.0));
        assert_eq!(reveal_axis(&(110.0..200.0), 100.0, 50.0), Some(110.0));
    }

    #[test]
    fn ignores_results_from_other_queries_or_viewers() {
        let mut state = super::super::tests::state();
        let doc = Document::from_bytes(Path::new("notes.txt"), b"old new".to_vec());
        let _ = state.update(Message::Loaded(2, Ok(Arc::new(doc))));
        let _ = state.update(Message::FindOpen);
        let _ = state.update(Message::FindQuery("old".into()));
        let old = state.find.generation;
        let _ = state.update(Message::FindQuery("new".into()));
        let current = state.find.generation;
        let hits = matches(state.code().expect("loaded text"), false, "new");
        for (request, generation) in [(1, current), (2, old)] {
            let result = state.update(Message::FindResults(request, generation, hits.clone()));
            assert!(result.output.is_none());
            assert!(state.find.pending);
        }
        assert!(matches!(
            state.update(Message::FindResults(2, current, hits)).output,
            Some(Output::RevealMatch)
        ));
        assert!(!state.find.pending);
    }
}
