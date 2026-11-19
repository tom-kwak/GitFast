//! Shared read-only code dialog. Callers supply a path and a file version;
//! this feature owns the loaded text, its viewport and its selection.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gf_git::{FileVersion, Repository};
use iced::advanced::mouse::{Click, click};
use iced::mouse::Button;
use iced::widget::text::Wrapping;
use iced::widget::{column, container, mouse_area, row};
use iced::{Background, Border, Element, Length, Padding, Task};
use unicode_segmentation::UnicodeSegmentation;

use crate::feature::{Cx, Feature, Update};
use crate::features::context_menu::{Entry, Menu};
use crate::highlight::{self, Spans};
use crate::nav::{self, Key};
use crate::theme::Tokens;
use crate::widgets::selectable_line::{Geometry, TextPointer, selectable_line};
use crate::widgets::virtual_list::{ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim, format};

const LIST_ID: &str = "gf-code-view";
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
    spans: Option<Vec<Spans>>,
}

impl Document {
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
                widest =
                    widest.max(columns(line) + 3 * line.bytes().filter(|&b| b == b'\t').count());
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
            lines,
            widest,
            spans,
        })
    }
}

impl Code {
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
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(u64, Result<Arc<Document>, String>),
    Scrolled(ScrollPos),
    Pressed(usize, TextPointer),
    Extend(usize, TextPointer),
    Dragged(usize, TextPointer),
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
    ) -> (Self, Task<Message>) {
        let task = Task::perform(load(repo, path.clone(), version.clone()), move |result| {
            Message::Loaded(request, result)
        });
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
        };
        (state, task)
    }

    fn code(&self) -> Option<&Code> {
        match self.loaded.as_ref()?.as_ref().ok()?.as_ref() {
            Document::Text(code) => Some(code),
            Document::Binary => None,
        }
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    fn press(&mut self, index: usize, pointer: TextPointer, extend: bool) {
        let Some(code) = self.code().filter(|code| index < code.lines.len()) else {
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
        let line = row_height(t);
        let page = (self.scroll.height - line).max(line);
        match key {
            Key::Up => nav::scroll_by(LIST_ID, -line),
            Key::Down => nav::scroll_by(LIST_ID, line),
            Key::PageUp => nav::scroll_by(LIST_ID, -page),
            Key::PageDown => nav::scroll_by(LIST_ID, page),
            Key::Home => nav::scroll_y(LIST_ID, 0.0),
            Key::End => iced::widget::operation::snap_to_end(LIST_ID),
            Key::Left | Key::Right => iced::widget::operation::scroll_by(
                LIST_ID,
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

    fn lines<'a>(&'a self, t: &'a Tokens, code: &'a Code) -> Element<'a, Message> {
        let digits = code.lines.len().to_string().len().max(3);
        let code_x = t.cols(digits + 3);
        let selection = self.selection();
        let list = virtual_list(
            LIST_ID,
            code.lines.len(),
            row_height(t),
            t.cols(digits + code.widest + 5),
            self.scroll,
            move |index| {
                let raw = code.line(index);
                let text = match code.spans.as_ref().and_then(|spans| spans.get(index)) {
                    Some(spans) => widgets::highlighted_line(t, spans),
                    None => widgets::mono(t, format::code_line(raw))
                        .wrapping(Wrapping::None)
                        .into(),
                };
                let line = row![
                    container(dim(t, format!("{:>digits$}", index + 1)))
                        .width(Length::Fixed(t.cols(digits + 1))),
                    iced::widget::space().width(Length::Fixed(t.char_width())),
                    iced::widget::space().width(Length::Fixed(t.char_width())),
                    text,
                ]
                .align_y(iced::Alignment::Center);
                selectable_line(
                    container(line).width(Length::Fill).height(Length::Fill),
                    raw,
                    Geometry {
                        marker_x: code_x - t.char_width(),
                        code_x,
                        cell: t.char_width(),
                        size: t.font_size,
                    },
                    move |pointer| Message::Pressed(index, pointer),
                )
                .selected(
                    code.selection_on(index, selection.clone())
                        .into_iter()
                        .collect(),
                    t.diff_selection,
                    ' ',
                )
                .on_drag(self.dragging, move |pointer| {
                    Message::Dragged(index, pointer)
                })
                .into()
            },
            Message::Scrolled,
        );
        if self.dragging {
            mouse_area(list)
                .on_release(Message::DragEnd)
                .on_exit(Message::DragEnd)
                .into()
        } else {
            mouse_area(list).into()
        }
    }
}

fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.6).round()
}

async fn load(
    repo: Repository,
    path: PathBuf,
    version: FileVersion,
) -> Result<Arc<Document>, String> {
    let bytes = repo
        .file_bytes(&path, &version)
        .await
        .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || Arc::new(Document::from_bytes(&path, bytes)))
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
            Message::Scrolled(scroll) => self.scroll = scroll,
            Message::Pressed(line, pointer) if pointer.button == Button::Right => {
                let Some(code) = self.code().filter(|code| line < code.lines.len()) else {
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
            Message::Pressed(line, pointer) => self.press(line, pointer, false),
            Message::Extend(line, pointer) => self.press(line, pointer, true),
            Message::Dragged(line, pointer) if self.dragging => {
                if let Some(code) = self.code().filter(|code| line < code.lines.len()) {
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
                self.head = self.code().map_or(0, |code| code.text.len());
                self.anchor = 0;
                self.dragging = false;
                self.last_click = None;
            }
            Message::Copy => {
                let selection = self.selection();
                if let Some(code) = self.code().filter(|_| !selection.is_empty()) {
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
        let header = row![
            colored(t, "FILE CONTENTS", t.accent()),
            dim(t, version).width(Length::Fill),
            widgets::text_button(
                t,
                "Copy",
                Some("⌘C"),
                (!self.selection().is_empty()).then_some(Message::Copy)
            ),
            widgets::text_button(t, "Close", Some("Esc"), Some(Message::Close)),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        let body = match &self.loaded {
            None => widgets::empty_hint(t, "Loading file…"),
            Some(Err(error)) => colored(t, error.as_str(), t.error()).into(),
            Some(Ok(doc)) => match doc.as_ref() {
                Document::Binary => {
                    widgets::empty_hint(t, "Preview is available for UTF-8 text files")
                }
                Document::Text(code) if code.lines.is_empty() => {
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
                    .style(crate::theme::style::panel(t)),
            ]
            .spacing(8),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .max_width(1200)
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
        let _ = state.update(Message::Pressed(0, pointer));
        let _ = state.update(Message::DragEnd);
        let _ = state.update(Message::Pressed(0, pointer));
        assert!(
            matches!(state.update(Message::Copy).output, Some(Output::Copy(text)) if text == "한글")
        );
        let _ = state.update(Message::Dragged(1, pointer));
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
