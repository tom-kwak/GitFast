//! Three-pane conflict resolver. The center is the worktree draft and changes
//! reach Git only when the user saves and stages the file.

mod model;
mod view;

use std::path::PathBuf;
use std::sync::Arc;

use gf_git::conflict::{self, Choice};
use gf_git::{ConflictDocument, Side};
use iced::Task;
use iced::widget::text_editor;

pub use model::Prepared;
use model::{AlignedResult, Layout, Resolution};

use crate::nav;
use crate::theme::Tokens;
use crate::widgets::virtual_list::ScrollPos;

const LIST: &str = "conflict-aligned-rows";
const EDITOR: &str = "conflict-result-editor";

#[derive(Debug)]
pub struct State {
    pub path: PathBuf,
    pub request: u64,
    file: Option<Arc<Prepared>>,
    resolutions: Vec<Option<Resolution>>,
    remaining: usize,
    history: Vec<Vec<Option<Resolution>>>,
    result: Vec<Vec<String>>,
    aligned: Vec<Vec<AlignedResult>>,
    layout: Layout,
    active: usize,
    difference: Option<usize>,
    scroll: ScrollPos,
    base_scroll: ScrollPos,
    file_scroll: ScrollPos,
    edit: Option<text_editor::Content>,
    loading: bool,
    error: Option<String>,
    changed: bool,
    saving: bool,
    resolved_on_disk: bool,
    refresh_error: Option<String>,
    show_files: bool,
    file_actions: bool,
    block_actions: bool,
    show_base: bool,
    expanded: bool,
    narrow_side: Side,
    focused_row: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Choose(usize, Choice),
    ReplaceChoice(usize, Choice),
    Undo,
    Apply,
    Edit,
    EditAction(text_editor::Action),
    AcceptEdit,
    CancelEdit,
    SelectBlock(usize),
    Difference(bool),
    NextUnresolved,
    Scrolled(ScrollPos),
    BaseScrolled(ScrollPos),
    FilesScrolled(ScrollPos),
    ToggleFiles,
    ToggleFileActions,
    ToggleBlockActions,
    ToggleBase,
    ToggleContext,
    SwitchSource,
    OpenFile(PathBuf),
    Reload,
    TakeFile(Side),
    MarkResolved,
    Close,
}

#[derive(Debug)]
pub enum Output {
    Apply(PathBuf, Arc<ConflictDocument>, String),
    OpenFile(PathBuf),
    NextFile,
    ReplaceChoice(usize, Choice),
    Reload(PathBuf),
    TakeFile(PathBuf, Side),
    MarkResolved(PathBuf),
    Close,
}

impl State {
    pub fn loading(path: PathBuf) -> Self {
        Self {
            path,
            request: 0,
            file: None,
            resolutions: Vec::new(),
            remaining: 0,
            history: Vec::new(),
            result: Vec::new(),
            aligned: Vec::new(),
            layout: Layout::default(),
            active: 0,
            difference: None,
            scroll: ScrollPos::default(),
            base_scroll: ScrollPos::default(),
            file_scroll: ScrollPos::default(),
            edit: None,
            loading: true,
            error: None,
            changed: false,
            saving: false,
            resolved_on_disk: false,
            refresh_error: None,
            show_files: true,
            file_actions: false,
            block_actions: false,
            show_base: false,
            expanded: false,
            narrow_side: Side::Ours,
            focused_row: None,
        }
    }

    pub fn reload(&mut self, request: u64, waiting: bool) {
        self.request = request;
        self.loading = waiting || self.file.is_none();
    }

    pub fn loaded(&mut self, incoming: Result<Arc<Prepared>, String>) -> bool {
        self.loading = false;
        let file = match incoming {
            Ok(file) => file,
            Err(error) => {
                self.error = Some(error);
                return false;
            }
        };
        self.error = None;
        if self.resolved_on_disk {
            self.file = None;
            self.edit = None;
            self.resolutions.clear();
            self.resolved_on_disk = false;
        }
        if self
            .file
            .as_ref()
            .is_some_and(|old| old.original == file.original)
        {
            self.changed = false;
            return false;
        }
        if self
            .file
            .as_ref()
            .is_some_and(|old| old.original.same_index(&file.original))
            && self
                .candidate()
                .as_deref()
                .is_some_and(|text| file.original.worktree.as_deref() == Some(text))
        {
            // A failed git add can leave the prepared result on disk. Keep
            // the draft and allow the same operation to retry staging it.
            return false;
        }
        if self.file.is_some() && self.has_draft() {
            self.changed = true;
            return false;
        }
        self.resolutions = vec![None; file.blocks.len()];
        self.remaining = file.blocks.len();
        self.history = vec![Vec::new(); file.blocks.len()];
        self.result = vec![Vec::new(); file.blocks.len()];
        self.aligned = file
            .blocks
            .iter()
            .map(|block| block.align_result(&[], None))
            .collect();
        self.file = Some(file);
        self.changed = false;
        self.active = 0;
        self.rebuild();
        true
    }

    pub fn has_draft(&self) -> bool {
        !self.resolved_on_disk
            && (self.edit.is_some() || self.resolutions.iter().any(Option::is_some))
    }

    pub fn unresolved(&self) -> Option<usize> {
        if self.resolved_on_disk {
            return Some(0);
        }
        let file = self.file.as_ref()?;
        file.segments.as_ref()?;
        Some(self.remaining)
    }

    pub fn saving(&mut self, saving: bool) {
        self.saving = saving;
        if saving {
            self.refresh_error = None;
        }
    }

    pub fn editing(&self) -> bool {
        self.edit.is_some()
    }

    pub fn resolved_on_disk(&mut self) {
        self.resolved_on_disk = true;
        self.loading = false;
        self.error = None;
        self.refresh_error = None;
        self.edit = None;
        self.changed = false;
    }

    pub fn resolved_outside_app(&mut self) {
        if self.has_draft() {
            self.changed = true;
        } else {
            self.resolved_on_disk();
        }
    }

    pub fn operation_failed(&mut self, error: &str) {
        self.refresh_error = Some(error.into());
    }

    pub fn restore_scroll(&self) -> Task<Message> {
        nav::scroll_y(LIST, self.scroll.offset)
    }

    pub fn key(&self, key: nav::Key) -> Option<Message> {
        if self.editing() {
            return None;
        }
        match key {
            nav::Key::Char('p') => Some(Message::Difference(true)),
            nav::Key::Char('n') => Some(Message::Difference(false)),
            nav::Key::Char('j') => Some(Message::NextUnresolved),
            nav::Key::Char('e') => Some(Message::Edit),
            _ => None,
        }
    }

    fn candidate(&self) -> Option<String> {
        model::candidate(self.file.as_ref()?, &self.resolutions)
    }

    fn can_apply(&self) -> bool {
        !self.loading
            && !self.saving
            && !self.resolved_on_disk
            && !self.changed
            && self.error.is_none()
            && self.edit.is_none()
            && !self.resolutions.is_empty()
            && self.unresolved() == Some(0)
    }

    fn rebuild(&mut self) {
        if let Some(file) = &self.file {
            self.layout = Layout::new(file, self.expanded, &self.aligned);
        }
    }

    fn record(&mut self, block: usize, choice: Option<Resolution>) {
        if let Some(slot) = self.resolutions.get_mut(block)
            && *slot != choice
        {
            self.remaining += usize::from(choice.is_none());
            self.remaining -= usize::from(slot.is_none());
            self.history[block].push(slot.take());
            *slot = choice;
            self.refresh_result(block);
        }
    }

    fn refresh_result(&mut self, block: usize) {
        let text = self
            .file
            .as_ref()
            .zip(self.resolutions[block].as_ref())
            .and_then(|(file, choice)| file.resolved_text(block, choice))
            .unwrap_or_default();
        self.result[block] = text.lines().map(str::to_owned).collect();
        if let Some(source) = self.file.as_ref().and_then(|file| file.blocks.get(block)) {
            self.aligned[block] =
                source.align_result(&self.result[block], self.resolutions[block].as_ref());
        }
        self.rebuild();
    }

    fn jump(&mut self, row: usize, t: &Tokens) -> Task<Message> {
        let line = self.edit_line(row);
        if let Some(editor) = self.edit.as_mut()
            && let Some(line) = line
        {
            editor.move_to(text_editor::Cursor {
                position: text_editor::Position {
                    line: line.min(editor.line_count().saturating_sub(1)),
                    column: 0,
                },
                selection: None,
            });
        }
        self.focused_row = Some(row);
        #[allow(clippy::cast_precision_loss)]
        let offset = row.saturating_sub(2) as f32 * view::row_height(t);
        self.scroll.offset = offset;
        nav::scroll_y(LIST, offset)
    }

    fn edit_line(&self, row: usize) -> Option<usize> {
        let model::Row::Code { block, index } = *self.layout.rows.get(row)? else {
            return None;
        };
        let active = self.aligned.get(block)?.get(index)?;
        if self.resolutions.get(block)?.is_some() {
            active.result
        } else {
            let source = self
                .file
                .as_ref()?
                .blocks
                .get(block)?
                .pairs
                .get(active.source?)?;
            source.ours.or(source.theirs)
        }
    }

    pub fn reveal(&mut self, t: &Tokens) -> Task<Message> {
        if let Some(row) = self.layout.headings.get(self.active).copied() {
            self.jump(row, t)
        } else {
            self.scroll.offset = 0.0;
            nav::scroll_y(LIST, 0.0)
        }
    }

    fn select(&mut self, block: usize, t: &Tokens) -> Task<Message> {
        if block >= self.resolutions.len() || self.edit.is_some() {
            return Task::none();
        }
        self.active = block;
        self.difference = None;
        self.reveal(t)
    }

    pub fn update(&mut self, m: Message, t: &Tokens) -> (Task<Message>, Option<Output>) {
        let output = match m {
            Message::Choose(block, choice)
                if !self.resolved_on_disk
                    && !self.changed
                    && self.edit.is_none()
                    && !self.saving =>
            {
                if matches!(
                    self.resolutions.get(block),
                    Some(Some(Resolution::Edited(_)))
                ) {
                    Some(Output::ReplaceChoice(block, choice))
                } else {
                    self.record(block, Some(Resolution::Side(choice)));
                    None
                }
            }
            Message::ReplaceChoice(block, choice)
                if !self.resolved_on_disk
                    && !self.changed
                    && self.edit.is_none()
                    && !self.saving =>
            {
                self.record(block, Some(Resolution::Side(choice)));
                None
            }
            Message::Undo if self.edit.is_none() && !self.saving => {
                if let Some(previous) = self.history.get_mut(self.active).and_then(Vec::pop) {
                    let slot = &mut self.resolutions[self.active];
                    self.remaining += usize::from(previous.is_none());
                    self.remaining -= usize::from(slot.is_none());
                    *slot = previous;
                    self.refresh_result(self.active);
                }
                None
            }
            Message::Apply if self.can_apply() => self
                .file
                .as_ref()
                .zip(self.candidate())
                .map(|(file, text)| Output::Apply(self.path.clone(), file.original.clone(), text)),
            Message::Edit if !self.resolved_on_disk && !self.changed && !self.saving => {
                if let Some(file) = &self.file
                    && let Some(block) = file.block(self.active)
                {
                    let text = self.resolutions[self.active]
                        .as_ref()
                        .and_then(|choice| file.resolved_text(self.active, choice))
                        .unwrap_or_else(|| block.ours.clone());
                    self.edit = Some(text_editor::Content::with_text(&text));
                    self.refresh_error = None;
                    let mut tasks = vec![iced::widget::operation::focus(EDITOR)];
                    if let Some(row) = self.focused_row {
                        tasks.push(self.jump(row, t));
                    }
                    return (Task::batch(tasks), None);
                }
                None
            }
            Message::EditAction(action) => {
                if let Some(edit) = &mut self.edit {
                    let scroll = if let text_editor::Action::Scroll { lines } = action {
                        Some(lines)
                    } else {
                        None
                    };
                    edit.perform(action);
                    if let Some(lines) = scroll {
                        #[allow(clippy::cast_precision_loss)]
                        let offset = self.scroll.offset + lines as f32 * view::row_height(t);
                        self.scroll.offset = offset.max(0.0);
                        return (nav::scroll_y(LIST, self.scroll.offset), None);
                    }
                }
                None
            }
            Message::AcceptEdit if !self.saving => {
                if let Some(edit) = self.edit.take() {
                    let mut text = edit.text();
                    if self.file.as_ref().is_some_and(|file| {
                        conflict::has_markers(&text, file.original.marker_width)
                    }) {
                        self.edit = Some(edit);
                        self.refresh_error =
                            Some("remove the remaining conflict markers from this block".into());
                        return (Task::none(), None);
                    }
                    self.refresh_error = None;
                    // A manually edited block must not consume the next
                    // unchanged line. Preserve the user's empty deletion.
                    let followed = self
                        .file
                        .as_ref()
                        .is_some_and(|file| file.has_following_content(self.active));
                    if followed && !text.is_empty() && !text.ends_with('\n') {
                        let crlf = self
                            .file
                            .as_ref()
                            .and_then(|file| file.original.worktree.as_deref())
                            .and_then(|worktree| worktree.split_inclusive('\n').next())
                            .is_some_and(|line| line.ends_with("\r\n"));
                        text.push_str(if crlf { "\r\n" } else { "\n" });
                    }
                    self.record(self.active, Some(Resolution::Edited(text)));
                }
                None
            }
            Message::CancelEdit => {
                self.edit = None;
                None
            }
            Message::SelectBlock(block) => return (self.select(block, t), None),
            Message::Difference(back) => {
                if let Some(rows) = self
                    .layout
                    .differences
                    .get(self.active)
                    .filter(|r| !r.is_empty())
                {
                    let next = self.difference.map_or_else(
                        || if back { rows.len() - 1 } else { 0 },
                        |i| {
                            if back {
                                i.saturating_sub(1).min(rows.len() - 1)
                            } else {
                                (i + 1).min(rows.len() - 1)
                            }
                        },
                    );
                    self.difference = Some(next);
                    return (self.jump(rows[next], t), None);
                }
                None
            }
            Message::NextUnresolved if self.edit.is_none() => {
                let next = self
                    .resolutions
                    .iter()
                    .enumerate()
                    .skip(self.active + 1)
                    .chain(self.resolutions.iter().enumerate().take(self.active))
                    .find_map(|(i, choice)| choice.is_none().then_some(i));
                if let Some(next) = next {
                    return (self.select(next, t), None);
                }
                Some(Output::NextFile)
            }
            Message::Scrolled(pos) => {
                self.scroll = pos;
                None
            }
            Message::BaseScrolled(pos) => {
                self.base_scroll = pos;
                None
            }
            Message::FilesScrolled(pos) => {
                self.file_scroll = pos;
                None
            }
            Message::ToggleFiles => {
                self.show_files = !self.show_files;
                None
            }
            Message::ToggleFileActions => {
                self.file_actions = !self.file_actions;
                None
            }
            Message::ToggleBlockActions => {
                self.block_actions = !self.block_actions;
                None
            }
            Message::ToggleBase => {
                self.show_base = !self.show_base;
                None
            }
            Message::ToggleContext => {
                self.expanded = !self.expanded;
                self.rebuild();
                return (self.reveal(t), None);
            }
            Message::SwitchSource => {
                self.narrow_side = match self.narrow_side {
                    Side::Ours => Side::Theirs,
                    Side::Theirs => Side::Ours,
                };
                None
            }
            Message::OpenFile(path) => Some(Output::OpenFile(path)),
            Message::Reload if self.changed && !self.saving => {
                Some(Output::Reload(self.path.clone()))
            }
            Message::TakeFile(side) if !self.saving => {
                Some(Output::TakeFile(self.path.clone(), side))
            }
            Message::MarkResolved if !self.saving && !self.changed => {
                Some(Output::MarkResolved(self.path.clone()))
            }
            Message::Close => Some(Output::Close),
            _ => None,
        };
        (Task::none(), output)
    }

    pub fn can_mark_resolved(&self) -> bool {
        self.file.as_ref().is_none_or(|f| {
            f.original
                .worktree
                .as_deref()
                .is_none_or(|text| !conflict::has_markers(text, f.original.marker_width))
        })
    }

    pub fn side_missing(&self, side: Side) -> bool {
        let slot = if side == Side::Ours { 1 } else { 2 };
        self.file
            .as_ref()
            .is_some_and(|file| !file.original.present[slot] && file.original.present[3 - slot])
    }
}
