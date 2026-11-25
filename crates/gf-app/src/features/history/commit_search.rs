//! State and result presentation for commit searches within the current graph.

use gf_core::model::{Commit, Oid};
use gf_git::{COMMIT_SEARCH_LIMIT, LogOrder, LogQuery};
use iced::Element;
use iced::widget::{column, responsive, rich_text, row, span};

use super::{State as History, revision_label, search};
use crate::theme::Tokens;
use crate::widgets;

/// The graph range and the snapshot version of its refs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub revision: Option<String>,
    pub all_branches: bool,
    pub refs_key: Option<u64>,
}

impl Scope {
    pub fn query(&self) -> LogQuery {
        LogQuery {
            revisions: self.revision.clone().into_iter().collect(),
            all_refs: self.all_branches && self.revision.is_none(),
            order: LogOrder::Topo,
            ..LogQuery::default()
        }
    }
}

#[derive(Debug, Default)]
pub struct State {
    pub generation: u64,
    pub cancel: Option<gf_git::CancellationToken>,
    pub hits: Vec<Hit>,
    pub error: Option<String>,
    pub truncated: bool,
    has_excerpts: bool,
    hash_columns: usize,
    pub locating: bool,
}

impl State {
    pub fn invalidate(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
        self.error = None;
        self.locating = false;
    }

    pub fn set_hits(&mut self, hits: Vec<Hit>) {
        self.has_excerpts = hits.iter().any(|hit| hit.excerpt.is_some());
        self.hash_columns = hits
            .iter()
            .map(|hit| hit.hash.text.len())
            .max()
            .unwrap_or(7);
        self.hits = hits;
    }

    pub fn clear(&mut self) {
        self.invalidate();
        self.set_hits(Vec::new());
        self.truncated = false;
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.invalidate();
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub id: Oid,
    hash: Line,
    title: Line,
    excerpt: Option<Line>,
}

#[derive(Debug, Clone)]
struct Line {
    text: String,
    matched: Option<std::ops::Range<usize>>,
}

impl Line {
    fn view<'a>(
        &self,
        t: &Tokens,
        columns: usize,
        ready: bool,
    ) -> iced::widget::text::Rich<'a, (), search::Message> {
        use crate::widgets::virtual_list::{char_columns, columns as text_columns};
        let (text, matched) = if text_columns(&self.text) <= columns {
            (self.text.clone(), self.matched.clone())
        } else {
            let mut remaining = columns.saturating_sub(1);
            let end = self
                .text
                .char_indices()
                .find_map(|(index, ch)| {
                    remaining
                        .checked_sub(char_columns(ch))
                        .map(|next| remaining = next)
                        .is_none()
                        .then_some(index)
                })
                .unwrap_or(self.text.len());
            let text = format!("{}…", &self.text[..end]);
            let matched = self
                .matched
                .clone()
                .filter(|range| range.start < end)
                .map(|range| range.start..range.end.min(end));
            (text, matched)
        };
        let pieces = match matched {
            Some(range) if ready => vec![
                span(text[..range.start].to_owned()),
                span(text[range.clone()].to_owned()).color(t.accent()),
                span(text[range.end..].to_owned()),
            ],
            _ => vec![span(text)],
        };
        rich_text(pieces)
            .font(crate::platform::mono_font())
            .color(if ready { t.fg } else { t.dim })
            .size(t.font_size)
            .wrapping(iced::widget::text::Wrapping::None)
    }
}

/// Map the match back to UTF-8 boundaries in the original line.
fn matched(line: &str, needle: &str) -> Option<std::ops::Range<usize>> {
    let lower: String = line.chars().flat_map(char::to_lowercase).collect();
    let start = lower.find(needle)?;
    let end = start + needle.len();
    let mut offset = 0;
    let mut first = None;
    for (i, ch) in line.char_indices() {
        offset += ch.to_lowercase().map(char::len_utf8).sum::<usize>();
        if offset > start && first.is_none() {
            first = Some(i);
        }
        if offset >= end {
            return Some(first?..i + ch.len_utf8());
        }
    }
    None
}

/// Trim around the matching text while keeping UTF-8 boundaries intact.
fn preview(text: &str, found: Option<std::ops::Range<usize>>, width: usize) -> Line {
    let start = found.as_ref().map_or(0, |range| {
        text[..range.start]
            .char_indices()
            .rev()
            .nth(width / 4)
            .map_or(0, |(i, ch)| i + ch.len_utf8())
    });
    let end = text[start..]
        .char_indices()
        .nth(width)
        .map_or(text.len(), |(i, _)| start + i);
    let prefix = if start == 0 { "" } else { "…" };
    let suffix = if end == text.len() { "" } else { "…" };
    Line {
        text: format!("{prefix}{}{suffix}", &text[start..end]),
        matched: found.map(|r| {
            r.start.min(end) - start + prefix.len()..r.end.min(end) - start + prefix.len()
        }),
    }
}

pub fn prepare(commits: Vec<Commit>, term: &str) -> Vec<Hit> {
    let needle: String = term.trim().chars().flat_map(char::to_lowercase).collect();
    let hash_query = needle.len() >= 4 && needle.bytes().all(|b| b.is_ascii_hexdigit());
    commits
        .iter()
        .take(COMMIT_SEARCH_LIMIT + 1)
        .map(|commit| {
            let id = commit.id.as_str();
            let hash_match = hash_query && id.starts_with(&needle);
            let mut hash_len = 7;
            while commits.iter().any(|other| {
                other.id != commit.id && other.id.as_str().starts_with(&id[..hash_len])
            }) {
                hash_len += 1;
            }
            let title_match = matched(&commit.summary, &needle);
            let excerpt = title_match
                .is_none()
                .then(|| {
                    commit.body.lines().find_map(|line| {
                        matched(line, &needle).map(|found| preview(line, Some(found), 100))
                    })
                })
                .flatten();
            Hit {
                id: commit.id.clone(),
                hash: Line {
                    text: id[..hash_len].to_owned(),
                    matched: hash_match.then_some(0..needle.len().min(hash_len)),
                },
                title: preview(&commit.summary, title_match, 120),
                excerpt,
            }
        })
        .collect()
}

impl History {
    pub fn search_row_height(&self, t: &Tokens) -> f32 {
        let height = search::row_height(t);
        if self.search_mode == search::Mode::Commits && self.commit_search.has_excerpts {
            height * 2.0
        } else {
            height
        }
    }

    pub(super) fn history_search_view<'a>(
        &'a self,
        t: &'a Tokens,
        all: bool,
        snap: &crate::repo_tab::Snapshot,
    ) -> Element<'a, super::Message> {
        let revision = self.revision.as_deref().map(revision_label);
        if self.search_mode == search::Mode::Files {
            return self.file_search_view(t, format!("files at {}", revision.unwrap_or("HEAD")));
        }
        let Some(search) = self.search_state() else {
            return widgets::empty_hint(t, "search is closed");
        };
        let range = revision.unwrap_or(if all { "all branches" } else { "HEAD" });
        let state = &self.commit_search;
        let status = if state.locating {
            "Finding commit in history…".into()
        } else if snap.status.head.oid.is_none()
            && revision.is_none()
            && (!all || (snap.history_all_branches && snap.history.commits.is_empty()))
        {
            "No commits yet".into()
        } else if search.query().trim().is_empty() {
            "Type a commit message or at least 4 hash characters".into()
        } else if search.is_ready() {
            format!("No matching commits on {range}")
        } else {
            "Searching…".into()
        };
        search
            .view(
                t,
                search::View {
                    mode: self.search_mode,
                    scope: format!("commits on {range}"),
                    status: Some(status),
                    error: state.error.as_deref(),
                    truncated: state.truncated,
                    height: self.search_row_height(t),
                    locating: state.locating,
                },
                move |i, ready| {
                    let hit = &state.hits[i];
                    let message = responsive(move |size| {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let columns = (size.width / t.cols(1)).floor() as usize;
                        let mut lines = column![hit.title.view(t, columns, ready)];
                        if let Some(excerpt) = &hit.excerpt {
                            lines = lines.push(excerpt.view(t, columns, ready));
                        }
                        lines.clip(true).into()
                    });
                    row![
                        hit.hash
                            .view(t, state.hash_columns, ready)
                            .width(iced::Length::Fixed(t.cols(state.hash_columns))),
                        message
                    ]
                    .spacing(10)
                    .clip(true)
                    .into()
                },
            )
            .map(super::Message::Search)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_the_original_unicode_text_and_retains_it_in_a_preview() {
        let text = format!("{} 한글 İSTANBUL here", "x".repeat(200));
        let needle = "i\u{307}stanbul";
        let line = preview(&text, matched(&text, needle), 40);
        assert!(line.text.starts_with('…'));
        assert_eq!(&line.text[line.matched.expect("match")], "İSTANBUL");
    }
}
