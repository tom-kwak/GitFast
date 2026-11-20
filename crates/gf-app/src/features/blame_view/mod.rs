//! Blame: who last changed each line of a file. Consecutive lines from the
//! same commit form a block (commit info shown once, colored per commit);
//! clicking a line finds that commit in the history (blame stays open).
//! Lines not committed yet (working tree blame) are not clickable.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gf_core::model::Oid;
use gf_git::BlameLine;
use iced::widget::text::Wrapping;
use iced::widget::{column, row};
use iced::{Element, Length, Task};

use crate::feature::Update;
use crate::highlight::{self, Spans};
use crate::theme::Tokens;
use crate::widgets::file_search::{self, Hit, Search, line_width};
use crate::widgets::virtual_list::{ScrollPos, virtual_list};
use crate::widgets::{self, colored, dim, format};

pub(crate) const FIND_FIELD_ID: &str = "gf-blame-find";
const LIST_ID: &str = "gf-blame";
const INFO_COLUMNS: usize = 34;
const ROW_GAP: f32 = 8.0;
const ROW_PADDING: f32 = 6.0;

#[derive(Debug, Clone)]
pub struct State {
    pub path: PathBuf,
    /// Revision blamed; `None` = the working tree file.
    pub rev: Option<String>,
    lines: Option<Arc<Vec<BlameLine>>>,
    /// Widest line, in columns (measured once, when the lines arrive).
    widest: usize,
    /// Syntax highlighting per line, filled in after the lines arrive.
    spans: Option<Arc<Vec<Spans>>>,
    error: Option<String>,
    scroll: ScrollPos,
    find: Search<()>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(Oid),
    Scrolled(ScrollPos),
    FindOpen,
    FindClose,
    FindQuery(String),
    FindMove { back: bool },
    FindResults(u64, Arc<[Hit<()>]>),
    Close,
}

#[derive(Debug, Clone)]
pub enum Output {
    ShowCommit(Oid),
    RevealMatch,
    Close,
}

/// Lane-palette index for a commit, so neighbouring blocks differ in color.
fn color_index(id: &Oid) -> usize {
    usize::from(id.as_str().as_bytes().first().copied().unwrap_or(0))
}

/// What hovering a committed line shows: the commit, who wrote it and when.
///
/// ```text
/// c956bdc · main 700
/// Kim Jihoon <jihoon@example.com>
/// 2023-11-15 09:53 · 2y ago
/// ```
fn hover(l: &BlameLine, now: i64) -> String {
    let who = if l.author_email.is_empty() {
        l.author.clone()
    } else {
        format!("{} <{}>", l.author, l.author_email)
    };
    let ago = u64::try_from(now - l.time.seconds).map_or_else(
        |_| "in the future".to_owned(),
        |s| match format::age(std::time::Duration::from_secs(s)).as_str() {
            "now" => "just now".to_owned(),
            a => format!("{a} ago"),
        },
    );
    format!(
        "{} · {}\n{who}\n{} · {ago}",
        l.commit.short(),
        l.summary,
        format::datetime(l.time)
    )
}

/// Files longer than this are shown without highlighting.
const MAX_HIGHLIGHT_LINES: usize = 20_000;

/// Highlights the blamed file in the background; `None` when no grammar
/// matches the path or the file is too long.
pub async fn highlight(path: PathBuf, lines: Arc<Vec<BlameLine>>) -> Option<Arc<Vec<Spans>>> {
    if lines.len() > MAX_HIGHLIGHT_LINES {
        return None;
    }
    tokio::task::spawn_blocking(move || {
        let text: Vec<_> = lines.iter().map(|line| line.content.as_str()).collect();
        highlight_lines(&path, &text)
    })
    .await
    .ok()
    .flatten()
    .map(Arc::new)
}

fn highlight_lines(path: &Path, lines: &[&str]) -> Option<Vec<Spans>> {
    highlight::get().highlight(path, lines)
}

impl State {
    pub fn loading(path: PathBuf, rev: Option<String>) -> Self {
        Self {
            path,
            rev,
            lines: None,
            widest: 0,
            spans: None,
            error: None,
            scroll: ScrollPos::default(),
            find: Search::default(),
        }
    }

    pub fn loaded(&mut self, path: &PathBuf, result: Result<Vec<BlameLine>, String>) {
        if &self.path != path {
            return;
        }
        match result {
            Ok(l) => {
                self.widest = l.iter().map(|l| line_width(&l.content)).max().unwrap_or(0);
                self.lines = Some(Arc::new(l));
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// The blamed lines, once loaded (for highlighting).
    pub fn lines(&self) -> Option<&Arc<Vec<BlameLine>>> {
        self.lines.as_ref()
    }

    pub fn highlighted(&mut self, path: &PathBuf, spans: Arc<Vec<Spans>>) {
        let fits = self.lines.as_ref().is_some_and(|l| l.len() == spans.len());
        if &self.path == path && fits {
            self.spans = Some(spans);
        }
    }

    pub fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message {
            Message::Select(id) => return Update::output(Output::ShowCommit(id)),
            Message::Scrolled(p) => self.scroll = p,
            Message::FindOpen if self.can_find() => {
                let task = if self.find.open() {
                    self.search()
                } else {
                    Task::none()
                };
                return Update::task(Task::batch([task, file_search::focus(FIND_FIELD_ID)]));
            }
            Message::FindOpen => {}
            Message::FindClose => self.find.close(),
            Message::FindQuery(query) => {
                self.find.set_query(query);
                return Update::task(self.search());
            }
            Message::FindMove { back } => {
                if self.find.move_match(back) {
                    return Update::output(Output::RevealMatch);
                }
            }
            Message::FindResults(generation, hits) => {
                if self.find.accept(generation, hits) {
                    return Update::output(Output::RevealMatch);
                }
            }
            Message::Close => return Update::output(Output::Close),
        }
        Update::none()
    }

    fn can_find(&self) -> bool {
        self.lines.as_ref().is_some_and(|lines| !lines.is_empty())
    }

    pub fn find_is_open(&self) -> bool {
        self.find.is_open()
    }

    fn search(&mut self) -> Task<Message> {
        let Some(lines) = self.lines.clone() else {
            return Task::none();
        };
        let Some((generation, query)) = self.find.restart() else {
            return Task::none();
        };
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    file_search::matches(
                        &query,
                        lines
                            .iter()
                            .enumerate()
                            .map(|(row, line)| (row, (), line.content.as_str())),
                    )
                })
                .await
                .unwrap_or_default()
            },
            move |hits| Message::FindResults(generation, hits),
        )
    }

    fn number_columns(&self) -> usize {
        self.lines
            .as_ref()
            .and_then(|lines| lines.last())
            .map_or(5, |line| line.line.to_string().len().max(5))
    }

    pub fn reveal_match(&self, t: &Tokens) -> Task<Message> {
        let Some((hit, lines)) = self.find.active_match().zip(self.lines.as_ref()) else {
            return Task::none();
        };
        let code_x = t.cols(INFO_COLUMNS + self.number_columns()) + 2.0 * ROW_GAP + ROW_PADDING;
        file_search::reveal_match(
            hit,
            &lines[hit.row as usize].content,
            t,
            LIST_ID,
            code_x,
            row_height(t),
        )
    }

    pub fn view<'a>(&'a self, t: &'a Tokens) -> Element<'a, Message> {
        let at = self.rev.as_deref().map_or_else(
            || "working tree".to_owned(),
            |r| r.chars().take(7).collect(),
        );
        let mut header = row![
            colored(t, format!("{} @ {at}", self.path.display()), t.fg)
                .wrapping(Wrapping::None)
                .width(Length::Fill),
            dim(t, "click a line to find its commit in the history"),
        ];
        if self.can_find() && !self.find_is_open() {
            header = header.push(dim(t, "Find in file ⌘F"));
        }
        let header = header
            .push(widgets::icon_button(
                t,
                "✕",
                t.dim,
                "close blame",
                Message::Close,
            ))
            .spacing(6)
            .align_y(iced::Alignment::Center);

        let spans = self.spans.as_deref();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        let body: Element<'a, Message> = match (&self.lines, &self.error) {
            (_, Some(e)) => colored(t, format!("  ⚠ {e}"), t.error()).into(),
            (None, None) => widgets::empty_hint(t, "running blame…"),
            (Some(lines), None) if lines.is_empty() => widgets::empty_hint(t, "empty file"),
            (Some(lines), None) => {
                let digits = self.number_columns();
                let content_width =
                    t.cols(self.widest + INFO_COLUMNS + digits) + 2.0 * (ROW_GAP + ROW_PADDING);
                virtual_list(
                    LIST_ID,
                    lines.len(),
                    row_height(t),
                    content_width,
                    self.scroll,
                    move |i| {
                        let l = &lines[i];
                        let first_of_block = i == 0 || lines[i - 1].commit != l.commit;
                        let uncommitted = l.commit.is_null();
                        let info: Element<'a, Message> = if first_of_block && uncommitted {
                            dim(t, "not committed yet").into()
                        } else if first_of_block {
                            row![
                                colored(t, l.commit.short(), t.lane(color_index(&l.commit))),
                                dim(t, format::truncate_columns(&l.author, 12))
                                    .wrapping(Wrapping::None)
                                    .width(Length::Fixed(t.cols(12))),
                                dim(
                                    t,
                                    format::datetime(l.time)
                                        .chars()
                                        .take(10)
                                        .collect::<String>()
                                )
                                .wrapping(Wrapping::None)
                                .width(Length::Fixed(t.cols(10))),
                            ]
                            .spacing(6)
                            .into()
                        } else {
                            colored(t, "│", t.lane(color_index(&l.commit))).into()
                        };
                        let line = row![
                            iced::widget::container(info)
                                .width(Length::Fixed(t.cols(INFO_COLUMNS))),
                            dim(t, format!("{:>digits$}", l.line)).width(t.cols(digits)),
                            file_search::code_text(
                                t,
                                &l.content,
                                spans.and_then(|s| s.get(i)),
                                self.find.row_hits(i, ())
                            ),
                        ]
                        .spacing(ROW_GAP);
                        if uncommitted {
                            // Nothing to find: a local change, not a commit.
                            return iced::widget::container(line)
                                .padding(iced::Padding::from([1.0, ROW_PADDING]))
                                .width(Length::Fill)
                                .into();
                        }
                        let row =
                            widgets::list_row(t, line, false, Message::Select(l.commit.clone()));
                        // Every committed line says who wrote it, not only
                        // the first of its block. The row is as wide as the
                        // longest line, so the tip follows the mouse instead
                        // of sitting under the row's middle.
                        widgets::tip_at_cursor(t, row, hover(l, now))
                    },
                    Message::Scrolled,
                )
            }
        };
        let mut content = column![header].spacing(4);
        if self.find_is_open() {
            content = content.push(self.find.view(
                t,
                FIND_FIELD_ID,
                Message::FindQuery,
                |back| Message::FindMove { back },
                Message::FindClose,
            ));
        }
        widgets::accent_panel(t, "BLAME".into(), t.remote(), content.push(body))
    }
}

fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.6).round()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_known_languages_only() {
        let _ = highlight::init(None);
        let lines = ["fn main() {}", "let x = 1;"];
        let spans = highlight_lines(Path::new("src/main.rs"), &lines).expect("rust grammar");
        assert_eq!(spans.len(), 2);
        assert!(spans[0].len() > 1, "keyword and name get separate spans");
        assert!(highlight_lines(Path::new("notes.unknown-ext"), &lines).is_none());
    }

    fn line(author: &str, email: &str, seconds: i64) -> BlameLine {
        BlameLine {
            commit: "c956bdcabe00fc3430c440c570037b38a77d8676"
                .parse()
                .expect("oid"),
            line: 1,
            content: String::new(),
            author: author.into(),
            author_email: email.into(),
            time: gf_core::model::Timestamp {
                seconds,
                offset_minutes: 0,
            },
            summary: "main 700".into(),
        }
    }

    #[test]
    fn hover_says_the_commit_who_and_when() {
        let at = 1_700_000_000; // 2023-11-14 22:13 UTC
        let year = 365 * 86_400;
        assert_eq!(
            hover(
                &line("Kim Jihoon", "jihoon@example.com", at),
                at + 2 * year + 86_400
            ),
            "c956bdc · main 700\nKim Jihoon <jihoon@example.com>\n2023-11-14 22:13 · 2y ago"
        );
        let h = hover(&line("Kim", "", at), at + 3);
        assert!(h.contains("\nKim\n"), "no empty <>: {h}");
        assert!(h.ends_with("just now"), "{h}");
        assert!(
            hover(&line("Kim", "", at), at - 60).ends_with("in the future"),
            "clock skew"
        );
    }

    #[test]
    fn stale_or_mismatched_spans_are_ignored() {
        let mut s = State::loading("a.rs".into(), None);
        s.loaded(&"a.rs".into(), Ok(Vec::new()));
        s.highlighted(&"b.rs".into(), Arc::new(Vec::new()));
        assert!(s.spans.is_none(), "other file");
        s.highlighted(&"a.rs".into(), Arc::new(vec![Vec::new()]));
        assert!(s.spans.is_none(), "line count differs");
        s.highlighted(&"a.rs".into(), Arc::new(Vec::new()));
        assert!(s.spans.is_some());
    }
}
