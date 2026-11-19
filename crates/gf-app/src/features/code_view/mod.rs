//! Shared read-only code dialog. Callers supply a path and a file version;
//! this feature owns the loaded text, its viewport and its selection.

mod comparison;
mod render;
mod scroll_marks;

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gf_core::model::{FileChange, FileDiff, LineKind};
use gf_git::{FileVersion, Repository};
use iced::advanced::mouse::{Click, click};
use iced::mouse::Button;
use iced::widget::{button, column, container, row};
use iced::{Background, Border, Element, Length, Padding, Task};
use unicode_segmentation::UnicodeSegmentation;

use crate::feature::{Cx, Feature, Update};
use crate::features::context_menu::{Entry, Menu};
use crate::highlight::{self, Spans};
use crate::nav::{self, Key};
use crate::theme::{Tokens, style};
use crate::widgets::selectable_line::TextPointer;
use crate::widgets::virtual_list::{ScrollPos, columns};
use crate::widgets::{self, colored, dim};

const LIST_ID: &str = "gf-code-view";
const BEFORE_LIST: &str = "gf-code-before";
const AFTER_LIST: &str = "gf-code-after";
const COMPARE_GUTTER: &str = "gf-code-compare-gutter";
const COMPARE_LISTS: [&str; 3] = [BEFORE_LIST, COMPARE_GUTTER, AFTER_LIST];
const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 20_000;

#[derive(Debug)]
pub enum Document {
    Text(Code),
    Binary,
}

#[derive(Debug)]
pub struct Code {
    text: String,
    lines: Vec<Range<usize>>,
    widest: usize,
    max_line_number: usize,
    spans: Option<Vec<Spans>>,
    preview_rows: Option<Vec<PreviewRow>>,
    scroll_marks: Arc<[scroll_marks::Mark]>,
    comparison: Option<comparison::Comparison>,
}

type PrepareComparison = Box<dyn FnOnce(&mut Document, Option<Code>) + Send>;

/// The old file to read and the diff already loaded by the caller.
pub struct ComparisonRequest {
    pub before: Option<(PathBuf, FileVersion)>,
    pub labels: [&'static str; 2],
    pub prepare: PrepareComparison,
}

#[derive(Debug, PartialEq, Eq)]
enum PreviewRow {
    File(usize, LineKind),
    Removed { number: u32, text: String },
}

impl Document {
    /// Decorate the full file with the diff already shown by the caller.
    pub fn show_changes(&mut self, diff: &FileDiff, before: Option<Code>) {
        let Self::Text(code) = self else { return };
        code.comparison = comparison::Comparison::new(code, before, diff);
        let Some(rows) = code.preview_rows(diff) else {
            return;
        };
        for row in &rows {
            if let PreviewRow::Removed { number, text } = row {
                code.widest = code.widest.max(line_width(text));
                code.max_line_number = code.max_line_number.max(*number as usize);
            }
        }
        code.scroll_marks = scroll_marks::collect(rows.iter().map(|row| match row {
            PreviewRow::File(_, kind) => [*kind == LineKind::Added, *kind == LineKind::Removed],
            PreviewRow::Removed { .. } => [false, true],
        }));
        code.preview_rows = Some(rows);
    }

    fn from_bytes(path: &Path, bytes: Vec<u8>) -> Self {
        if bytes.contains(&0) {
            return Self::Binary;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            return Self::Binary;
        };
        let mut offset = 0;
        let mut widest = 0;
        let lines: Vec<_> = text
            .split_inclusive('\n')
            .map(|raw| {
                let start = offset;
                offset += raw.len();
                let line = raw.strip_suffix('\n').unwrap_or(raw);
                let line = line.strip_suffix('\r').unwrap_or(line);
                widest = widest.max(line_width(line));
                start..start + line.len()
            })
            .collect();
        let spans = if !lines.is_empty()
            && text.len() <= MAX_HIGHLIGHT_BYTES
            && lines.len() <= MAX_HIGHLIGHT_LINES
        {
            let source: Vec<_> = lines.iter().map(|line| &text[line.clone()]).collect();
            highlight::get().highlight(path, &source)
        } else {
            None
        };
        Self::Text(Code {
            text,
            max_line_number: lines.len(),
            lines,
            widest,
            spans,
            preview_rows: None,
            scroll_marks: Arc::default(),
            comparison: None,
        })
    }
}

impl Code {
    fn preview_rows(&self, diff: &FileDiff) -> Option<Vec<PreviewRow>> {
        if diff.is_binary || diff.hunks.is_empty() {
            return None;
        }
        // A deleted file is opened at its previous version. Its real
        // lines are still selectable and copyable in the preview.
        if diff.change == FileChange::Deleted {
            return Some(
                (0..self.lines.len())
                    .map(|i| PreviewRow::File(i, LineKind::Removed))
                    .collect(),
            );
        }
        let mut rows = Vec::with_capacity(self.lines.len());
        let mut next = 0;
        for hunk in &diff.hunks {
            // With zero new lines, Git gives the preceding line number.
            let start = if hunk.new_lines == 0 {
                hunk.new_start
            } else {
                hunk.new_start.checked_sub(1)?
            } as usize;
            if !(next..=self.lines.len()).contains(&start) {
                return None;
            }
            rows.extend((next..start).map(|i| PreviewRow::File(i, LineKind::Context)));
            next = start;
            for line in &hunk.lines {
                if line.kind == LineKind::Removed {
                    rows.push(PreviewRow::Removed {
                        number: line.old_lineno?,
                        text: line.content.trim_end_matches('\r').to_owned(),
                    });
                } else {
                    if next >= self.lines.len()
                        || (line.kind == LineKind::Added
                            && self.line(next) != line.content.trim_end_matches('\r'))
                    {
                        return None;
                    }
                    rows.push(PreviewRow::File(next, line.kind));
                    next += 1;
                }
            }
        }
        rows.extend((next..self.lines.len()).map(|i| PreviewRow::File(i, LineKind::Context)));
        Some(rows)
    }

    fn line(&self, index: usize) -> &str {
        &self.text[self.lines[index].clone()]
    }

    fn byte_at(&self, index: usize, position: usize) -> usize {
        let line = &self.lines[index];
        // The shared line widget reserves position zero for a diff marker.
        let column = position.saturating_sub(1);
        self.line(index)
            .char_indices()
            .nth(column)
            .map_or(line.end, |(offset, _)| line.start + offset)
    }

    fn selection_on(&self, index: usize, selection: Range<usize>) -> Option<(usize, usize, bool)> {
        let line = &self.lines[index];
        if selection.is_empty() || selection.end <= line.start || selection.start > line.end {
            return None;
        }
        let column = |offset: usize| {
            let offset = offset.clamp(line.start, line.end);
            self.text[line.start..offset].chars().count() + 1
        };
        Some((
            column(selection.start),
            column(selection.end),
            selection.end > line.end,
        ))
    }
}

/// State for a single opening of the dialog, identified by its request ID.
#[derive(Debug)]
pub struct State {
    request: u64,
    path: PathBuf,
    version: FileVersion,
    loaded: Option<Result<Arc<Document>, String>>,
    scroll: ScrollPos,
    anchor: usize,
    head: usize,
    dragging: bool,
    last_click: Option<(usize, Click)>,
    comparison_labels: Option<[&'static str; 2]>,
    split: bool,
    active_pane: Pane,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Before,
    #[default]
    After,
}

impl Pane {
    fn list_id(self) -> &'static str {
        match self {
            Self::Before => BEFORE_LIST,
            Self::After => AFTER_LIST,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(u64, Result<Arc<Document>, String>),
    Scrolled(ScrollPos),
    CompareScrolled(&'static str, ScrollPos),
    SetSplit(bool),
    Pressed(Pane, usize, TextPointer),
    Extend(Pane, usize, TextPointer),
    Dragged(Pane, usize, TextPointer),
    DragEnd,
    SelectAll,
    Copy,
    Close,
}

#[derive(Debug)]
pub enum Output {
    Copy(String),
    Menu(Menu<Message>),
    Close,
}

impl State {
    /// Starts loading a file without blocking the app or publishing its contents to the log.
    pub fn open(
        request: u64,
        repo: Repository,
        path: PathBuf,
        version: FileVersion,
        comparison: Option<ComparisonRequest>,
    ) -> (Self, Task<Message>) {
        let comparison_labels = comparison.as_ref().map(|request| request.labels);
        let task = Task::perform(
            load(repo, path.clone(), version.clone(), comparison),
            move |result| Message::Loaded(request, result),
        );
        let state = Self {
            request,
            path,
            version,
            loaded: None,
            scroll: ScrollPos::default(),
            anchor: 0,
            head: 0,
            dragging: false,
            last_click: None,
            comparison_labels,
            split: false,
            active_pane: Pane::After,
        };
        (state, task)
    }

    fn code(&self) -> Option<&Code> {
        match self.loaded.as_ref()?.as_ref().ok()?.as_ref() {
            Document::Text(code) => Some(code),
            Document::Binary => None,
        }
    }

    fn pane_code(&self, pane: Pane) -> Option<&Code> {
        let code = self.code()?;
        if !self.split {
            return Some(code);
        }
        let comparison = code.comparison.as_ref()?;
        match pane {
            Pane::Before if comparison.deleted => Some(code),
            Pane::Before => comparison.before.as_deref(),
            Pane::After if comparison.deleted => None,
            Pane::After => Some(code),
        }
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    fn activate_pane(&mut self, pane: Pane) {
        if self.active_pane != pane {
            self.anchor = 0;
            self.head = 0;
            self.dragging = false;
            self.last_click = None;
            self.active_pane = pane;
        }
    }

    fn press(&mut self, pane: Pane, index: usize, pointer: TextPointer, extend: bool) {
        let extend = extend && self.active_pane == pane;
        self.activate_pane(pane);
        let Some(code) = self.pane_code(pane).filter(|code| index < code.lines.len()) else {
            return;
        };
        let previous = self
            .last_click
            .filter(|(line, _)| *line == index)
            .map(|(_, click)| click);
        let click = Click::new(pointer.point, pointer.button, previous);
        let at = code.byte_at(index, pointer.caret);
        let (anchor, head) = if extend {
            (self.anchor, at)
        } else {
            match click.kind() {
                click::Kind::Single => (at, at),
                click::Kind::Double => {
                    let line = &code.lines[index];
                    let hit = code.byte_at(index, pointer.hit) - line.start;
                    code.line(index)
                        .split_word_bound_indices()
                        .find(|(start, word)| *start <= hit && hit < *start + word.len())
                        .map_or((at, at), |(start, word)| {
                            let start = line.start + start;
                            (start, start + word.len())
                        })
                }
                click::Kind::Triple => {
                    let line = &code.lines[index];
                    (line.start, line.end)
                }
            }
        };
        self.anchor = anchor;
        self.head = head;
        self.last_click = (!extend).then_some((index, click));
        self.dragging = true;
    }

    /// Scroll keys apply to the dialog while it is open.
    pub fn scroll_key(&self, key: Key, t: &Tokens) -> Task<Message> {
        let id = if self.split {
            self.active_pane.list_id()
        } else {
            LIST_ID
        };
        let line = row_height(t);
        let page = (self.scroll.height - line).max(line);
        match key {
            Key::Up => nav::scroll_by(id, -line),
            Key::Down => nav::scroll_by(id, line),
            Key::PageUp => nav::scroll_by(id, -page),
            Key::PageDown => nav::scroll_by(id, page),
            Key::Home => nav::scroll_y(id, 0.0),
            Key::End if self.split => iced::widget::operation::snap_to(
                id,
                iced::widget::operation::RelativeOffset {
                    x: None,
                    y: Some(1.0),
                },
            ),
            Key::End => iced::widget::operation::snap_to_end(id),
            Key::Left | Key::Right => iced::widget::operation::scroll_by(
                id,
                iced::widget::scrollable::AbsoluteOffset {
                    x: if key == Key::Left {
                        -t.cols(4)
                    } else {
                        t.cols(4)
                    },
                    y: 0.0,
                },
            ),
            _ => Task::none(),
        }
    }
}

fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.6).round()
}

fn line_width(line: &str) -> usize {
    columns(line) + 3 * line.bytes().filter(|&byte| byte == b'\t').count()
}

fn rescale_scroll(mut scroll: ScrollPos, from: usize, to: usize) -> ScrollPos {
    let position = (scroll.offset / (scroll.content - scroll.height).max(1.0)).clamp(0.0, 1.0);
    #[allow(clippy::cast_precision_loss)] // Scroll dimensions are pixel estimates.
    let scale = to as f32 / from.max(1) as f32;
    scroll.content *= scale;
    scroll.offset = position * (scroll.content - scroll.height).max(0.0);
    scroll
}

async fn load(
    repo: Repository,
    path: PathBuf,
    version: FileVersion,
    comparison: Option<ComparisonRequest>,
) -> Result<Arc<Document>, String> {
    let before = comparison
        .as_ref()
        .and_then(|request| request.before.clone());
    let (bytes, before) = tokio::join!(repo.file_bytes(&path, &version), async {
        let (path, version) = before?;
        let bytes = repo.file_bytes(&path, &version).await.ok()?;
        Some((path, bytes))
    });
    let bytes = bytes.map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let mut document = Document::from_bytes(&path, bytes);
        if let Some(request) = comparison {
            let before =
                before.and_then(|(path, bytes)| match Document::from_bytes(&path, bytes) {
                    Document::Text(code) => Some(code),
                    Document::Binary => None,
                });
            (request.prepare)(&mut document, before);
        }
        Arc::new(document)
    })
    .await
    .map_err(|e| e.to_string())
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message {
            Message::Loaded(request, result) if request == self.request => {
                self.loaded = Some(result);
            }
            Message::Loaded(..) => {}
            Message::Scrolled(scroll) if !self.split => self.scroll = scroll,
            Message::CompareScrolled(source, scroll) if self.split => {
                let moved = (scroll.offset - self.scroll.offset).abs() >= 0.5;
                self.scroll = scroll;
                if moved {
                    return Update::task(Task::batch(
                        COMPARE_LISTS
                            .into_iter()
                            .filter(|id| *id != source)
                            .map(|id| nav::scroll_y(id, scroll.offset)),
                    ));
                }
            }
            Message::Scrolled(_) | Message::CompareScrolled(..) => {}
            Message::SetSplit(split) if self.split != split => {
                let Some(code) = self.code() else {
                    return Update::none();
                };
                let Some(comparison) = code.comparison.as_ref() else {
                    return Update::none();
                };
                let pane = if split && comparison.deleted {
                    Pane::Before
                } else {
                    Pane::After
                };
                let inline_rows = code
                    .preview_rows
                    .as_ref()
                    .map_or(code.lines.len(), Vec::len);
                let split_rows = comparison.rows.len();
                let (from, to) = if split {
                    (inline_rows, split_rows)
                } else {
                    (split_rows, inline_rows)
                };
                self.scroll = rescale_scroll(self.scroll, from, to);
                self.activate_pane(pane);
                self.split = split;
                self.dragging = false;
                self.last_click = None;
                let offset = self.scroll.offset;
                let task = if split {
                    Task::batch(COMPARE_LISTS.map(|id| nav::scroll_y(id, offset)))
                } else {
                    nav::scroll_y(LIST_ID, offset)
                };
                return Update::task(task);
            }
            Message::SetSplit(_) => {}
            Message::Pressed(pane, line, pointer) if pointer.button == Button::Right => {
                self.activate_pane(pane);
                let Some(code) = self.pane_code(pane).filter(|code| line < code.lines.len()) else {
                    return Update::none();
                };
                let hit = code.byte_at(line, pointer.hit);
                if !self.selection().contains(&hit) {
                    self.anchor = hit;
                    self.head = hit;
                }
                self.last_click = None;
                self.dragging = false;
                let mut entries = Vec::new();
                if !self.selection().is_empty() {
                    entries.push(Entry::item('c', "Copy", "⌘C", Message::Copy));
                }
                entries.push(Entry::item('a', "Select All", "⌘A", Message::SelectAll));
                return Update::output(Output::Menu(Menu {
                    title: "File contents".into(),
                    entries,
                }));
            }
            Message::Pressed(pane, line, pointer) => self.press(pane, line, pointer, false),
            Message::Extend(pane, line, pointer) => self.press(pane, line, pointer, true),
            Message::Dragged(pane, line, pointer) if self.dragging && self.active_pane == pane => {
                if let Some(code) = self.pane_code(pane).filter(|code| line < code.lines.len()) {
                    let head = code.byte_at(line, pointer.caret);
                    if head != self.head {
                        self.head = head;
                        self.last_click = None;
                    }
                }
            }
            Message::Dragged(..) => {}
            Message::DragEnd => self.dragging = false,
            Message::SelectAll => {
                self.head = self
                    .pane_code(self.active_pane)
                    .map_or(0, |code| code.text.len());
                self.anchor = 0;
                self.dragging = false;
                self.last_click = None;
            }
            Message::Copy => {
                let selection = self.selection();
                if let Some(code) = self
                    .pane_code(self.active_pane)
                    .filter(|_| !selection.is_empty())
                {
                    return Update::output(Output::Copy(code.text[selection].to_owned()));
                }
            }
            Message::Close => return Update::output(Output::Close),
        }
        Update::none()
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let version = match &self.version {
            FileVersion::WorkTree => "Working tree",
            FileVersion::Index => "Staged",
            FileVersion::Revision(rev) => rev,
        };
        let mut header = row![
            colored(t, "FILE CONTENTS", t.accent()),
            dim(t, version).width(Length::Fill),
        ];
        if self.code().is_some_and(|code| code.comparison.is_some()) {
            let (color, hint) = if self.split {
                (t.accent(), "Show inline changes")
            } else {
                (t.dim, "Show side-by-side comparison")
            };
            header = header.push(widgets::tip(
                t,
                button(widgets::mono(t, "[ Side by side ]"))
                    .padding(Padding::from([1, 2]))
                    .style(style::text_button(t, color))
                    .on_press(Message::SetSplit(!self.split)),
                hint.into(),
            ));
        }
        let header = header
            .push(widgets::icon_button(
                t,
                "✕",
                t.dim,
                "Close · Esc",
                Message::Close,
            ))
            .spacing(8)
            .align_y(iced::Alignment::Center);
        let body = match &self.loaded {
            None => widgets::empty_hint(t, "Loading file…"),
            Some(Err(error)) => colored(t, error.as_str(), t.error()).into(),
            Some(Ok(doc)) => match doc.as_ref() {
                Document::Binary => {
                    widgets::empty_hint(t, "Preview is available for UTF-8 text files")
                }
                Document::Text(code)
                    if self.split
                        && let Some(comparison) = &code.comparison
                        && let Some(labels) = self.comparison_labels =>
                {
                    self.comparison_view(t, comparison, labels)
                }
                Document::Text(code) if code.lines.is_empty() && code.preview_rows.is_none() => {
                    widgets::empty_hint(t, "Empty file")
                }
                Document::Text(code) => self.lines(t, code),
            },
        };
        let dialog = container(
            column![
                header,
                widgets::mono(t, self.path.display().to_string()).width(Length::Fill),
                container(body)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(1)
                    .style(crate::theme::style::panel(t)),
            ]
            .spacing(8),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .max_width(if self.comparison_labels.is_some() {
            1600
        } else {
            1200
        })
        .max_height(840)
        .padding(Padding::from([10, 14]))
        .style(move |_| container::Style {
            text_color: Some(t.fg),
            background: Some(Background::Color(t.surface)),
            border: Border {
                color: t.accent(),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        });
        container(dialog).padding(24).center(Length::Fill).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            request: 2,
            path: "notes.txt".into(),
            version: FileVersion::WorkTree,
            loaded: None,
            scroll: ScrollPos::default(),
            anchor: 0,
            head: 0,
            dragging: false,
            last_click: None,
            comparison_labels: None,
            split: false,
            active_pane: Pane::After,
        }
    }

    #[test]
    fn selection_copies_unicode_and_original_line_endings() {
        let mut state = state();
        let original = "a 한글\r\n\tfinal\n";
        let document = Document::from_bytes(&state.path, original.as_bytes().to_vec());
        let _ = state.update(Message::Loaded(2, Ok(Arc::new(document))));
        let pointer = TextPointer {
            button: Button::Left,
            caret: 3,
            hit: 3,
            point: iced::Point::ORIGIN,
        };
        let _ = state.update(Message::Pressed(Pane::After, 0, pointer));
        let _ = state.update(Message::DragEnd);
        let _ = state.update(Message::Pressed(Pane::After, 0, pointer));
        assert!(
            matches!(state.update(Message::Copy).output, Some(Output::Copy(text)) if text == "한글")
        );
        let _ = state.update(Message::Dragged(Pane::After, 1, pointer));
        assert!(
            matches!(state.update(Message::Copy).output, Some(Output::Copy(text)) if text == "한글\r\n\tf")
        );
        let _ = state.update(Message::SelectAll);
        assert!(
            matches!(state.update(Message::Copy).output, Some(Output::Copy(text)) if text == original)
        );
    }

    #[test]
    fn a_previous_open_cannot_replace_the_current_document() {
        let mut state = state();
        let _ = state.update(Message::Loaded(1, Ok(Arc::new(Document::Binary))));
        assert!(state.loaded.is_none());
        let _ = state.update(Message::Loaded(2, Err("current".into())));
        let _ = state.update(Message::Loaded(1, Ok(Arc::new(Document::Binary))));
        assert!(matches!(state.loaded, Some(Err(ref error)) if error == "current"));
    }

    #[test]
    fn recognizes_binary_and_empty_files() {
        for bytes in [vec![0], vec![0xff]] {
            assert!(matches!(
                Document::from_bytes(Path::new("file"), bytes),
                Document::Binary
            ));
        }
        assert!(
            matches!(Document::from_bytes(Path::new("file"), Vec::new()), Document::Text(code) if code.lines.is_empty())
        );
    }
}
