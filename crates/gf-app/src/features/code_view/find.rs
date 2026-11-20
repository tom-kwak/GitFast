//! Maps file viewer rows and comparison panes to the shared file search.

use std::sync::Arc;

use iced::Task;

use super::{Code, Document, LIST_ID, Message, Output, Pane, PreviewRow, State, row_height};
use crate::feature::Update;
use crate::theme::Tokens;
use crate::widgets::file_search::{self, Hit};

pub(crate) const FIELD_ID: &str = "gf-code-find";

impl State {
    /// Whether Escape belongs to the search bar.
    pub(crate) fn find_is_open(&self) -> bool {
        self.find.is_open()
    }

    pub(super) fn open_find(&mut self) -> Update<Message, Output> {
        if self.code().is_none() {
            return Update::none();
        }
        let task = if self.find.open() {
            self.search()
        } else {
            Task::none()
        };
        Update::task(Task::batch([task, file_search::focus(FIELD_ID)]))
    }

    pub(super) fn search(&mut self) -> Task<Message> {
        let Some(Ok(document)) = self.loaded.as_ref() else {
            return Task::none();
        };
        let Some((generation, query)) = self.find.restart() else {
            return Task::none();
        };
        let document = document.clone();
        let split = self.split;
        let request = self.request;
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
        hits: Arc<[Hit<Pane>]>,
    ) -> Update<Message, Output> {
        if self.request == request && self.find.accept(generation, hits) {
            Update::output(Output::RevealMatch)
        } else {
            Update::none()
        }
    }

    /// Reveal the active match using the target pane's actual viewport dimensions.
    pub(crate) fn reveal_match(&self, t: &Tokens) -> Task<Message> {
        let Some((hit, code)) = self.find.active_match().zip(self.code()) else {
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
        file_search::reveal_match(hit, raw, t, list, t.cols(gutter), row_height(t))
    }
}

fn inline_text(code: &Code, row: usize) -> &str {
    match code.preview_rows.as_ref().map(|rows| &rows[row]) {
        Some(PreviewRow::Removed { text, .. }) => text,
        Some(PreviewRow::File(index, _)) => code.line(*index),
        None => code.line(row),
    }
}

fn matches(code: &Code, split: bool, query: &str) -> Arc<[Hit<Pane>]> {
    if split && let Some(comparison) = &code.comparison {
        let before = if comparison.deleted {
            Some(code)
        } else {
            comparison.before.as_deref()
        };
        file_search::matches(
            query,
            comparison
                .rows
                .iter()
                .enumerate()
                .flat_map(|(row, aligned)| {
                    [
                        before
                            .zip(aligned.before)
                            .map(|(source, i)| (row, Pane::Before, source.line(i))),
                        aligned.after.map(|i| (row, Pane::After, code.line(i))),
                    ]
                    .into_iter()
                    .flatten()
                }),
        )
    } else {
        let len = code
            .preview_rows
            .as_ref()
            .map_or(code.lines.len(), Vec::len);
        file_search::matches(
            query,
            (0..len).map(|row| (row, Pane::After, inline_text(code, row))),
        )
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
    fn ignores_results_from_other_queries_or_viewers() {
        let mut state = super::super::tests::state();
        let doc = Document::from_bytes(Path::new("notes.txt"), b"old new".to_vec());
        let _ = state.update(Message::Loaded(2, Ok(Arc::new(doc))));
        let _ = state.update(Message::FindOpen);
        state.find.set_query("old".into());
        let (old, _) = state.find.restart().expect("open query");
        state.find.set_query("new".into());
        let (current, _) = state.find.restart().expect("open query");
        let hits = matches(state.code().expect("loaded text"), false, "new");
        for (request, generation) in [(1, current), (2, old)] {
            let result = state.update(Message::FindResults(request, generation, hits.clone()));
            assert!(result.output.is_none());
            assert!(state.find.active_match().is_none());
        }
        assert!(matches!(
            state.update(Message::FindResults(2, current, hits)).output,
            Some(Output::RevealMatch)
        ));
        assert!(state.find.active_match().is_some());
    }
}
