//! Find File in the History panel: which commit's tree is searched, how a
//! picked file opens its history, and the search view. The state lives in
//! [`State`]; this is its glue to [`search`] and [`file_search`].

use std::sync::Arc;

use gf_core::model::Oid;
use iced::Element;
use iced::widget::Row;

use super::{Message, State, file_search, search};
use crate::repo_tab::Snapshot;
use crate::theme::Tokens;
use crate::widgets::{self, colored};

impl State {
    /// A branch or tag resolves to its tip as of the snapshot (no git call);
    /// the whole history uses the snapshot's `HEAD`.
    pub fn file_search_base(&self, snap: &Snapshot) -> Option<Oid> {
        if self.file_revision.is_some() {
            return None;
        }
        let Some(revision) = self.revision.as_deref() else {
            return snap.status.head.oid.clone();
        };
        snap.branches
            .iter()
            .find(|b| b.name.as_str() == revision)
            .map(|b| b.target.clone())
            .or_else(|| {
                snap.tags
                    .iter()
                    .find(|t| t.name.as_str() == revision)
                    .map(|t| t.target.clone())
            })
            .or_else(|| {
                self.data
                    .as_ref()?
                    .commits
                    .first()
                    .map(|commit| commit.id.clone())
            })
    }

    /// The path picked in Find File, while its history is shown: the commit
    /// diff opens on this file first.
    pub fn searched_file(&self) -> Option<&std::path::Path> {
        self.file_history_origin.as_ref()?;
        self.file_revision.as_ref().map(|(path, _)| path.as_path())
    }

    pub fn open_file_search(&mut self, oid: Option<Oid>) -> Option<file_search::Work> {
        self.search = Some(search::State::default());
        self.file_search.open(oid)
    }

    pub fn close_file_search(&mut self) {
        self.search = None;
        self.file_search.close();
    }

    pub fn file_search_is_open(&self) -> bool {
        self.search.is_some()
    }

    pub fn file_search_query(&mut self, query: String) -> Option<file_search::FilterJob> {
        self.search.as_mut()?.query_changed(query.clone());
        self.file_search.query_changed(query)
    }

    pub fn file_search_paths_loaded(
        &mut self,
        request: u64,
        oid: &Oid,
        result: Result<Arc<file_search::Index>, String>,
    ) -> Option<file_search::FilterJob> {
        self.file_search.paths_loaded(request, oid, result)
    }

    pub fn file_search_selection(&mut self, row: usize) -> Option<(std::path::PathBuf, Oid)> {
        self.search.as_mut()?.activate(row)?;
        Some((
            self.file_search.path_for_hit(row)?.to_path_buf(),
            self.file_search.current_oid()?.clone(),
        ))
    }

    pub fn file_search_filter_finished(
        &mut self,
        request: u64,
        generation: u64,
        oid: &Oid,
        hits: Vec<file_search::Hit>,
    ) -> file_search::FilterCompletion {
        let completion = self
            .file_search
            .filter_finished(request, generation, oid, hits);
        if completion.accepted
            && let Some(search) = self.search.as_mut()
        {
            search.results(completion.result_count);
        }
        completion
    }

    pub fn search_state(&self) -> Option<&search::State> {
        self.search.as_ref()
    }

    #[cfg(test)]
    pub fn file_search_state(&self) -> &file_search::State {
        &self.file_search
    }

    /// Commits of the loaded revision (not the snapshot's).
    #[cfg(test)]
    pub fn loaded_commits(&self) -> Option<&[gf_core::model::Commit]> {
        self.data.as_ref().map(|d| d.commits.as_slice())
    }

    pub fn search_state_mut(&mut self) -> Option<&mut search::State> {
        self.search.as_mut()
    }

    pub(super) fn file_search_view<'a>(&'a self, t: &'a Tokens) -> Element<'a, Message> {
        let Some(search) = self.search.as_ref() else {
            return widgets::empty_hint(t, "file search is closed");
        };
        search
            .view(
                t,
                "file name or path…",
                "files",
                self.file_search.status_text(),
                |index, _selected| {
                    let label = self.file_search.label_for_hit(index).unwrap_or_default();
                    let marks = self.file_search.marks_for_hit(index).unwrap_or_default();
                    // A long path loses folders at the front: the file name
                    // (and what matched in it) stays in view.
                    let (shown, skipped) = truncate_front(label, 140);
                    // One text per run of matched / unmatched characters, not
                    // one per character: long paths stay cheap to lay out.
                    let mut line = Row::new().spacing(0);
                    let mut run = String::new();
                    let mut run_marked = false;
                    for (i, ch) in shown.chars().enumerate() {
                        let marked = (skipped == 0 || i > 0) && marks.contains(&(i + skipped));
                        if marked != run_marked && !run.is_empty() {
                            let color = if run_marked { t.accent() } else { t.fg };
                            line = line.push(colored(t, std::mem::take(&mut run), color));
                        }
                        run_marked = marked;
                        run.push(ch);
                    }
                    if !run.is_empty() {
                        let color = if run_marked { t.accent() } else { t.fg };
                        line = line.push(colored(t, run, color));
                    }
                    if skipped == 0 {
                        line.into()
                    } else {
                        widgets::tip(
                            t,
                            line,
                            self.file_search
                                .path_for_hit(index)
                                .map(|path| path.display().to_string())
                                .unwrap_or_default(),
                        )
                    }
                },
            )
            .map(Message::Search)
    }
}

/// `s` cut to `max` columns by dropping characters at the front (shown as
/// `…`), and how many characters of `s` the `…` replaced beyond itself:
/// character `i` of the result is character `i + skipped` of `s`.
fn truncate_front(s: &str, max: usize) -> (std::borrow::Cow<'_, str>, usize) {
    use crate::widgets::virtual_list::{char_columns, columns};
    if columns(s) <= max {
        return (s.into(), 0);
    }
    let chars: Vec<char> = s.chars().collect();
    let mut used = 1; // the `…`
    let mut keep = chars.len();
    while keep > 0 && used + char_columns(chars[keep - 1]) <= max {
        used += char_columns(chars[keep - 1]);
        keep -= 1;
    }
    let mut out = String::from("…");
    out.extend(&chars[keep..]);
    (out.into(), keep - 1)
}

#[cfg(test)]
mod truncate_tests {
    use super::truncate_front;

    #[test]
    fn long_paths_keep_the_file_name_and_map_marks() {
        let (shown, skipped) = truncate_front("abcdef/name.rs", 8);
        assert_eq!(shown, "…name.rs");
        // `n` is char 1 of the result and char 7 of the path.
        assert_eq!(skipped + 1, 7);
        assert_eq!(truncate_front("short.rs", 8), ("short.rs".into(), 0));
    }
}
