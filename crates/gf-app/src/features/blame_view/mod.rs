//! Blame: who last changed each line of a file. Consecutive lines from the
//! same commit form a block (commit info shown once, colored per commit);
//! clicking a line opens that commit's changes.

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
            Ok(l) => self.lines = Some(l),
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
            dim(t, "click a line to see its commit"),
            widgets::icon_button(t, "✕", t.dim, "close blame", Message::Close),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        let spans = self.spans.as_deref();
        let body: Element<'a, Message> = match (&self.lines, &self.error) {
            (_, Some(e)) => colored(t, format!("  ⚠ {e}"), t.error()).into(),
            (None, None) => widgets::empty_hint(t, "running blame…"),
            (Some(lines), None) if lines.is_empty() => widgets::empty_hint(t, "empty file"),
            (Some(lines), None) => {
                let widest = lines.iter().map(|l| columns(&l.content)).max().unwrap_or(0);
                let content_width = t.cols(widest + 50) + 24.0;
                virtual_list(
                    "gf-blame",
                    lines.len(),
                    (t.font_size * 1.6).round(),
                    content_width,
                    self.scroll,
                    move |i| {
                        let l = &lines[i];
                        let first_of_block = i == 0 || lines[i - 1].commit != l.commit;
                        let info: Element<'a, Message> = if first_of_block {
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
                                None => widgets::mono(t, l.content.replace('\t', "    "))
                                    .wrapping(Wrapping::None)
                                    .into(),
                            },
                        ]
                        .spacing(8);
                        let row =
                            widgets::list_row(t, line, false, Message::Select(l.commit.clone()));
                        if first_of_block {
                            widgets::tip(t, row, format!("{} · {}", l.commit.short(), l.summary))
                        } else {
                            row
                        }
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
