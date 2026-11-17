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
use iced::{Element, Length};

use crate::highlight::{self, Spans};
use crate::theme::Tokens;
use crate::widgets::virtual_list::{ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim, format};

#[derive(Debug, Clone)]
pub struct State {
    pub path: PathBuf,
    /// Revision blamed; `None` = the working tree file.
    pub rev: Option<String>,
    lines: Option<Vec<BlameLine>>,
    /// Widest line, in columns (measured once, when the lines arrive).
    widest: usize,
    /// Syntax highlighting per line, filled in after the lines arrive.
    spans: Option<Arc<Vec<Spans>>>,
    error: Option<String>,
    scroll: ScrollPos,
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(Oid),
    Scrolled(ScrollPos),
    Close,
}

#[derive(Debug, Clone)]
pub enum Output {
    ShowCommit(Oid),
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
pub async fn highlight(path: PathBuf, lines: Vec<String>) -> Option<Arc<Vec<Spans>>> {
    if lines.len() > MAX_HIGHLIGHT_LINES {
        return None;
    }
    tokio::task::spawn_blocking(move || highlight_lines(&path, &lines))
        .await
        .ok()
        .flatten()
        .map(Arc::new)
}

fn highlight_lines(path: &Path, lines: &[String]) -> Option<Vec<Spans>> {
    let text: Vec<&str> = lines.iter().map(String::as_str).collect();
    highlight::get().highlight(path, &text)
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
        }
    }

    pub fn loaded(&mut self, path: &PathBuf, result: Result<Vec<BlameLine>, String>) {
        if &self.path != path {
            return;
        }
        match result {
            Ok(l) => {
                self.widest = l.iter().map(|l| columns(&l.content)).max().unwrap_or(0);
                self.lines = Some(l);
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// The blamed lines, once loaded (for highlighting).
    pub fn lines(&self) -> Option<&[BlameLine]> {
        self.lines.as_deref()
    }

    pub fn highlighted(&mut self, path: &PathBuf, spans: Arc<Vec<Spans>>) {
        let fits = self.lines.as_ref().is_some_and(|l| l.len() == spans.len());
        if &self.path == path && fits {
            self.spans = Some(spans);
        }
    }

    pub fn update(&mut self, message: Message) -> Option<Output> {
        match message {
            Message::Select(id) => Some(Output::ShowCommit(id)),
            Message::Scrolled(p) => {
                self.scroll = p;
                None
            }
            Message::Close => Some(Output::Close),
        }
    }

    pub fn view<'a>(&'a self, t: &'a Tokens) -> Element<'a, Message> {
        let at = self.rev.as_deref().map_or_else(
            || "working tree".to_owned(),
            |r| r.chars().take(7).collect(),
        );
        let header = row![
            colored(t, format!("{} @ {at}", self.path.display()), t.fg)
                .wrapping(Wrapping::None)
                .width(Length::Fill),
            dim(t, "click a line to find its commit in the history"),
            widgets::icon_button(t, "✕", t.dim, "close blame", Message::Close),
        ]
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
                let content_width = t.cols(self.widest + 50) + 24.0;
                virtual_list(
                    "gf-blame",
                    lines.len(),
                    (t.font_size * 1.6).round(),
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
                                dim(t, format::truncate(&l.author, 12))
                                    .width(Length::Fixed(t.cols(12))),
                                dim(
                                    t,
                                    format::datetime(l.time)
                                        .chars()
                                        .take(10)
                                        .collect::<String>()
                                ),
                            ]
                            .spacing(6)
                            .into()
                        } else {
                            colored(t, "│", t.lane(color_index(&l.commit))).into()
                        };
                        let line = row![
                            iced::widget::container(info).width(Length::Fixed(t.cols(34))),
                            dim(t, format!("{:>5}", l.line)),
                            match spans.and_then(|s| s.get(i)) {
                                Some(s) => widgets::highlighted_line(t, s),
                                None => widgets::mono(t, format::code_line(&l.content))
                                    .wrapping(Wrapping::None)
                                    .into(),
                            },
                        ]
                        .spacing(8);
                        if uncommitted {
                            // Nothing to find: a local change, not a commit.
                            return iced::widget::container(line)
                                .padding(iced::Padding::from([1, 6]))
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
        widgets::accent_panel(
            t,
            "BLAME".into(),
            t.remote(),
            column![header, body].spacing(4),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_known_languages_only() {
        let _ = highlight::init(None);
        let lines = vec!["fn main() {}".to_owned(), "let x = 1;".to_owned()];
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
