//! Code change viewer. The app has two instances with different roles so
//! the user always knows what they are looking at:
//!
//! - [`Role::Local`] — **LOCAL CHANGES** (yellow): a file picked in the
//!   working tree. Editable: hunk and line stage/unstage/discard.
//! - [`Role::Commit`] — **COMMIT CHANGES** (blue): a commit or stash picked
//!   in the history/sidebar. Read-only, with a changed-files list
//!   ([`State::files_view`]) next to the diff.
//!
//! One file is shown at a time and its lines are virtualized, so even huge
//! diffs stay responsive. Syntax highlighting runs off the UI thread: the
//! first file when the diff loads, others when they are first selected.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use gf_core::model::{FileChange, FileDiff, LineKind, Oid};
use gf_git::{DiffOptions, DiffTarget, Repository, patch};
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row};
use iced::{Color, Element, Length, Padding, Task};

use crate::feature::{Cx, Update};
use crate::highlight::{self, Spans};
use crate::nav::{self, Key, Moved, Nav};
use crate::theme::{Tokens, style};
use crate::widgets::virtual_list::{ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim, mono};

/// Scrollable id of the commit's file list.
const FILES_ID: &str = "gf-commit-files";
/// Files larger than this (in diff lines) are shown without highlighting.
const MAX_HIGHLIGHT_LINES: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Local,
    Commit,
}

impl Role {
    pub fn title(self) -> &'static str {
        match self {
            Self::Local => "LOCAL CHANGES",
            Self::Commit => "COMMIT CHANGES",
        }
    }

    pub fn accent(self, t: &Tokens) -> Color {
        match self {
            Self::Local => t.warning(),
            Self::Commit => t.remote(),
        }
    }

    fn list_id(self) -> &'static str {
        match self {
            Self::Local => "gf-local-diff",
            Self::Commit => "gf-commit-diff",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSource {
    WorkTree(PathBuf),
    Staged(PathBuf),
    Untracked(PathBuf),
    Commit(Oid),
    Stash {
        index: usize,
        id: Oid,
    },
    /// Combined changes of several commits: `from` (the oldest one's parent,
    /// or the empty tree) to `to` (the newest). `contiguous` is false when
    /// the selection skips commits, whose changes are then included too.
    Range {
        from: String,
        to: Oid,
        count: usize,
        contiguous: bool,
    },
}

/// `git hash-object -t tree /dev/null`: the parent of a root commit.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

impl DiffSource {
    /// Work tree sources belong in the local view, commits/stashes in the
    /// commit view.
    pub fn role(&self) -> Role {
        match self {
            Self::WorkTree(_) | Self::Staged(_) | Self::Untracked(_) => Role::Local,
            Self::Commit(_) | Self::Stash { .. } | Self::Range { .. } => Role::Commit,
        }
    }

    fn target(&self) -> (DiffTarget, Vec<PathBuf>) {
        match self {
            Self::WorkTree(p) => (DiffTarget::WorkTree, vec![p.clone()]),
            Self::Staged(p) => (DiffTarget::Staged, vec![p.clone()]),
            Self::Untracked(p) => (DiffTarget::Untracked(p.clone()), Vec::new()),
            Self::Commit(oid) => (DiffTarget::Commit(oid.clone()), Vec::new()),
            Self::Stash { id, .. } => (
                DiffTarget::Range {
                    from: format!("{id}^1"),
                    to: id.to_string(),
                },
                Vec::new(),
            ),
            Self::Range { from, to, .. } => (
                DiffTarget::Range {
                    from: from.clone(),
                    to: to.to_string(),
                },
                Vec::new(),
            ),
        }
    }

    pub fn title(&self) -> String {
        match self {
            Self::WorkTree(p) => format!("{} · unstaged", p.display()),
            Self::Staged(p) => format!("{} · staged", p.display()),
            Self::Untracked(p) => format!("{} · new file", p.display()),
            Self::Commit(oid) => format!("commit {}", oid.short()),
            Self::Stash { index, .. } => format!("stash@{{{index}}}"),
            Self::Range {
                count, contiguous, ..
            } => {
                let note = if *contiguous {
                    ""
                } else {
                    " · includes commits in between"
                };
                format!("{count} commits combined{note}")
            }
        }
    }

    /// Partial operations available for this source.
    fn actions(&self) -> &'static [Action] {
        match self {
            Self::WorkTree(_) => &[Action::Stage, Action::Discard],
            Self::Staged(_) => &[Action::Unstage],
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Stage,
    Unstage,
    Discard,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Stage => "stage",
            Self::Unstage => "unstage",
            Self::Discard => "discard",
        }
    }

    /// `(cached, reverse)` for `git apply` — see `gf_git::patch`.
    pub fn apply_mode(self) -> (bool, bool) {
        match self {
            Self::Stage => (true, false),
            Self::Unstage => (true, true),
            Self::Discard => (false, true),
        }
    }
}

/// Highlighting of one file: `spans[hunk][line]`, `None` per line when no
/// grammar applies.
pub type FileSpans = Vec<Vec<Option<Spans>>>;

/// A loaded diff; `first_spans` is the highlighting of file 0.
#[derive(Debug, Default)]
pub struct LoadedDiff {
    pub files: Vec<FileDiff>,
    pub first_spans: Option<Arc<FileSpans>>,
}

/// Loads the diff for `source` and highlights its first file off the UI thread.
pub async fn load(
    repo: Repository,
    source: DiffSource,
    ignore_whitespace: bool,
) -> Result<Arc<LoadedDiff>, String> {
    let (target, paths) = source.target();
    let opts = DiffOptions {
        ignore_whitespace,
        paths,
        ..DiffOptions::default()
    };
    let files = repo.diff(&target, &opts).await.map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let first_spans = files.first().map(|f| Arc::new(highlight_file(f)));
        Arc::new(LoadedDiff { files, first_spans })
    })
    .await
    .map_err(|e| e.to_string())
}

/// Highlights one file in the background.
pub async fn highlight(file: FileDiff) -> Arc<FileSpans> {
    tokio::task::spawn_blocking(move || Arc::new(highlight_file(&file)))
        .await
        .unwrap_or_default()
}

/// Highlights each hunk twice — the old side (context + removed lines) and
/// the new side (context + added lines) — so each side is parsed as the
/// real file content, then picks the right result per line.
fn highlight_file(f: &FileDiff) -> FileSpans {
    let total: usize = f.hunks.iter().map(|h| h.lines.len()).sum();
    if f.is_binary || total > MAX_HIGHLIGHT_LINES {
        return f.hunks.iter().map(|h| vec![None; h.lines.len()]).collect();
    }
    let hl = highlight::get();
    let path = f.display_path().cloned().unwrap_or_default();
    f.hunks
        .iter()
        .map(|h| {
            let mut out: Vec<Option<Spans>> = vec![None; h.lines.len()];
            for keep in [LineKind::Removed, LineKind::Added] {
                let idx: Vec<usize> = h
                    .lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.kind == LineKind::Context || l.kind == keep)
                    .map(|(i, _)| i)
                    .collect();
                let text: Vec<&str> = idx.iter().map(|i| h.lines[*i].content.as_str()).collect();
                let Some(spans) = hl.highlight(&path, &text) else {
                    continue;
                };
                for (i, s) in idx.into_iter().zip(spans) {
                    // Context lines take the new side's result.
                    if h.lines[i].kind == keep || keep == LineKind::Added {
                        out[i] = Some(s);
                    }
                }
            }
            out
        })
        .collect()
}

/// `(file, hunk, line)` index of a selected line.
type LineKey = (usize, usize, usize);

/// One rendered row of the selected file.
#[derive(Debug, Clone, Copy)]
enum DiffRow {
    FileHeader,
    Notice(&'static str),
    Hunk(usize),
    Line(usize, usize),
}

#[derive(Debug)]
pub struct State {
    role: Role,
    source: Option<DiffSource>,
    diff: Arc<LoadedDiff>,
    /// Highlighting of files other than the first, loaded on selection.
    spans: HashMap<usize, Arc<FileSpans>>,
    /// Widest line of each file in monospace columns (for sideways scroll).
    widths: Vec<usize>,
    file: usize,
    loading: bool,
    error: Option<String>,
    selection: BTreeSet<LineKey>,
    ignore_whitespace: bool,
    scroll: ScrollPos,
    files_scroll: ScrollPos,
}

#[derive(Debug, Clone)]
pub enum Message {
    ToggleLine(LineKey),
    ClearSelection,
    Hunk { hunk: usize, action: Action },
    Selected(Action),
    SelectFile(usize),
    Scrolled(ScrollPos),
    FilesScrolled(ScrollPos),
    ToggleWhitespace,
    Close,
    Blame,
    FileHistory,
}

#[derive(Debug, Clone)]
pub enum Output {
    /// Blame the shown file at `rev` (`None` = working tree).
    Blame { path: PathBuf, rev: Option<String> },
    /// Show the commits that touched the shown file.
    FileHistory(PathBuf),
    /// Apply these patches in order. Discards must be confirmed first.
    Apply {
        patches: Vec<String>,
        action: Action,
        description: String,
    },
    /// Reload the current source (options changed).
    Reload,
    /// File `n` needs highlighting; the app calls [`highlight`] and
    /// [`State::highlighted`].
    Highlight(usize, Box<FileDiff>),
}

impl State {
    pub fn new(role: Role) -> Self {
        Self {
            role,
            source: None,
            diff: Arc::default(),
            spans: HashMap::new(),
            widths: Vec::new(),
            file: 0,
            loading: false,
            error: None,
            selection: BTreeSet::new(),
            ignore_whitespace: false,
            scroll: ScrollPos::default(),
            files_scroll: ScrollPos::default(),
        }
    }

    pub fn source(&self) -> Option<&DiffSource> {
        self.source.as_ref()
    }

    /// Whether showing `source` again would just repeat work: it is being
    /// loaded, or it is a commit/stash (immutable) that is already loaded.
    pub fn is_current(&self, source: &DiffSource) -> bool {
        self.source.as_ref() == Some(source)
            && (self.loading
                || (self.error.is_none()
                    && matches!(
                        source,
                        DiffSource::Commit(_) | DiffSource::Stash { .. } | DiffSource::Range { .. }
                    )))
    }

    /// Path of the file being shown (new side, or old side if deleted).
    pub fn current_path(&self) -> Option<PathBuf> {
        match &self.source {
            Some(DiffSource::WorkTree(p) | DiffSource::Staged(p) | DiffSource::Untracked(p)) => {
                Some(p.clone())
            }
            _ => self
                .diff
                .files
                .get(self.file)
                .and_then(|f| f.display_path().cloned()),
        }
    }

    pub fn ignore_whitespace(&self) -> bool {
        self.ignore_whitespace
    }

    /// Starts showing `source`; the app loads it and calls [`Self::loaded`].
    pub fn show(&mut self, source: DiffSource) {
        if self.source.as_ref() != Some(&source) {
            self.selection.clear();
            self.diff = Arc::default();
            self.spans.clear();
            self.widths.clear();
            self.file = 0;
            self.scroll = ScrollPos::default();
            self.files_scroll = ScrollPos::default();
        }
        self.source = Some(source);
        self.loading = true;
        self.error = None;
    }

    pub fn close(&mut self) {
        *self = Self {
            ignore_whitespace: self.ignore_whitespace,
            ..Self::new(self.role)
        };
    }

    pub fn loaded(&mut self, source: &DiffSource, result: Result<Arc<LoadedDiff>, String>) {
        if self.source.as_ref() != Some(source) {
            return; // the user moved on
        }
        self.loading = false;
        match result {
            Ok(diff) => {
                // Drop selections that no longer point at a changed line.
                self.selection.retain(|(f, h, l)| {
                    diff.files
                        .get(*f)
                        .and_then(|f| f.hunks.get(*h))
                        .and_then(|h| h.lines.get(*l))
                        .is_some_and(|l| l.kind != LineKind::Context)
                });
                if self.file >= diff.files.len() {
                    self.file = 0;
                }
                self.spans.clear();
                if let Some(s) = &diff.first_spans {
                    self.spans.insert(0, s.clone());
                }
                self.widths = diff
                    .files
                    .iter()
                    .map(|f| {
                        f.hunks
                            .iter()
                            .flat_map(|h| &h.lines)
                            .map(|l| columns(&l.content) + 3 * l.content.matches('\t').count())
                            .max()
                            .unwrap_or(0)
                    })
                    .collect();
                self.diff = diff;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn highlighted(&mut self, source: &DiffSource, file: usize, spans: Arc<FileSpans>) {
        if self.source.as_ref() == Some(source) {
            self.spans.insert(file, spans);
        }
    }

    fn patches(
        &self,
        keys: impl Iterator<Item = (usize, usize, Option<BTreeSet<usize>>)>,
        action: Action,
    ) -> Vec<String> {
        let (_, reverse) = action.apply_mode();
        keys.filter_map(|(f, h, lines)| {
            let file = self.diff.files.get(f)?;
            patch::hunk_patch(file, h, lines.as_ref(), reverse)
        })
        .collect()
    }

    fn selection_by_hunk(&self) -> Vec<(usize, usize, Option<BTreeSet<usize>>)> {
        let mut out: Vec<(usize, usize, Option<BTreeSet<usize>>)> = Vec::new();
        for &(f, h, l) in &self.selection {
            match out.last_mut() {
                Some((lf, lh, Some(lines))) if *lf == f && *lh == h => {
                    lines.insert(l);
                }
                _ => out.push((f, h, Some(BTreeSet::from([l])))),
            }
        }
        out
    }

    fn rows(f: &FileDiff) -> Vec<DiffRow> {
        let mut rows = vec![DiffRow::FileHeader];
        if f.is_binary {
            rows.push(DiffRow::Notice("binary file — no text diff"));
        } else if f.hunks.is_empty() {
            rows.push(DiffRow::Notice("no content changes"));
        }
        for (hi, h) in f.hunks.iter().enumerate() {
            rows.push(DiffRow::Hunk(hi));
            rows.extend((0..h.lines.len()).map(|li| DiffRow::Line(hi, li)));
        }
        rows
    }

    fn row_view<'a>(&'a self, t: &'a Tokens, f: &'a FileDiff, r: DiffRow) -> Element<'a, Message> {
        let fi = self.file;
        let actions = self
            .source
            .as_ref()
            .map(DiffSource::actions)
            .unwrap_or_default();
        let partial = !actions.is_empty() && patch::supports_partial(f);
        match r {
            DiffRow::FileHeader => {
                let path = f
                    .display_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let (added, removed) = f.line_stats();
                let change = match f.change {
                    FileChange::Added => "new file".to_owned(),
                    FileChange::Deleted => "deleted".to_owned(),
                    FileChange::Modified => "modified".to_owned(),
                    FileChange::Renamed { similarity } => format!(
                        "renamed from {} ({similarity}%)",
                        f.old_path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default()
                    ),
                    FileChange::Copied { similarity } => format!("copied ({similarity}%)"),
                };
                let mut header = row![
                    colored(t, format!("── {path}"), self.role.accent(t)).wrapping(Wrapping::None),
                    dim(t, change).wrapping(Wrapping::None),
                    colored(t, format!("+{added}"), t.added()),
                    colored(t, format!("−{removed}"), t.removed()),
                ]
                .spacing(8);
                if f.mode_changed() {
                    header = header.push(dim(
                        t,
                        format!(
                            "mode {} → {}",
                            f.old_mode.as_deref().unwrap_or("?"),
                            f.new_mode.as_deref().unwrap_or("?")
                        ),
                    ));
                }
                header.into()
            }
            DiffRow::Notice(text) => dim(t, format!("    {text}")).into(),
            DiffRow::Hunk(hi) => {
                let h = &f.hunks[hi];
                // Buttons first: they stay visible however wide the file is.
                let mut bar = row![].spacing(2).align_y(iced::Alignment::Center);
                if partial {
                    for &a in actions {
                        let msg = Message::Hunk {
                            hunk: hi,
                            action: a,
                        };
                        let label = format!("{} hunk", a.label());
                        bar = bar.push(if a == Action::Discard {
                            widgets::danger_button(t, &label, Some(msg))
                        } else {
                            widgets::text_button(t, &label, None, Some(msg))
                        });
                    }
                }
                bar = bar.push(
                    dim(
                        t,
                        format!(
                            "@@ -{},{} +{},{} @@ {}",
                            h.old_start, h.old_lines, h.new_start, h.new_lines, h.section
                        ),
                    )
                    .wrapping(Wrapping::None),
                );
                container(bar)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_y(iced::Alignment::Center)
                    .padding(Padding::from([0, 4]))
                    .style(style::bar(t))
                    .into()
            }
            DiffRow::Line(hi, li) => {
                let line = &f.hunks[hi].lines[li];
                let (marker, marker_color) = match line.kind {
                    LineKind::Context => (' ', t.dim),
                    LineKind::Added => ('+', t.added()),
                    LineKind::Removed => ('-', t.removed()),
                };
                let num =
                    |n: Option<u32>| n.map_or_else(|| "     ".to_owned(), |n| format!("{n:>5}"));
                // Syntax-highlighted content when a grammar matched,
                // otherwise colored by change kind.
                let spans = self
                    .spans
                    .get(&fi)
                    .and_then(|s| s.get(hi))
                    .and_then(|h| h.get(li));
                let content: Element<'a, Message> = match spans {
                    Some(Some(spans)) => widgets::highlighted_line(t, spans),
                    _ => {
                        let color = match line.kind {
                            LineKind::Context => t.fg,
                            LineKind::Added => t.added(),
                            LineKind::Removed => t.removed(),
                        };
                        colored(t, line.content.replace('\t', "    "), color)
                            .wrapping(Wrapping::None)
                            .into()
                    }
                };
                let mut text_row = row![
                    dim(t, num(line.old_lineno)),
                    dim(t, num(line.new_lineno)),
                    colored(t, format!(" {marker} "), marker_color),
                    content,
                ];
                if line.no_newline_at_eof {
                    text_row =
                        text_row.push(dim(t, "  ⏎̸ no newline at end").wrapping(Wrapping::None));
                }
                let key = (fi, hi, li);
                if partial && line.kind != LineKind::Context {
                    let selected = self.selection.contains(&key);
                    button(text_row)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .padding(0)
                        .style(style::diff_line_button(t, line.kind, selected))
                        .on_press(Message::ToggleLine(key))
                        .into()
                } else {
                    container(text_row)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(style::diff_line(t, line.kind))
                        .into()
                }
            }
        }
    }

    fn row_height(t: &Tokens) -> f32 {
        (t.font_size * 1.6).round()
    }

    fn files_row_height(t: &Tokens) -> f32 {
        (t.font_size * 1.7).round()
    }

    /// Keys for the commit's file list: arrows pick the file.
    pub fn files_nav(&self, nav: Nav, t: &Tokens) -> Option<Moved<Message>> {
        let h = Self::files_row_height(t);
        let len = self.diff.files.len();
        let i = nav::step(
            Some(self.file),
            len,
            nav.key,
            nav::page(self.files_scroll, h),
        )?;
        if i == self.file {
            return Some(Moved::stay());
        }
        Some(
            Moved::message(Message::SelectFile(i)).with_task(nav::reveal(
                FILES_ID,
                self.files_scroll,
                i,
                h,
            )),
        )
    }

    /// Keys for the diff itself: arrows and PgUp/PgDn scroll, Home/End
    /// jump, `n`/`p` go to the next/previous hunk.
    pub fn diff_nav(&self, nav: Nav, t: &Tokens) -> Option<Moved<Message>> {
        let id = self.role.list_id();
        let h = Self::row_height(t);
        let page = (self.scroll.height - h).max(h);
        let task = match nav.key {
            Key::Up => nav::scroll_by(id, -h),
            Key::Down => nav::scroll_by(id, h),
            Key::PageUp => nav::scroll_by(id, -page),
            Key::PageDown => nav::scroll_by(id, page),
            Key::Home => nav::scroll_y(id, 0.0),
            Key::End => iced::widget::operation::snap_to_end(id),
            Key::Char(c @ ('n' | 'p')) => {
                let f = self.diff.files.get(self.file)?;
                let tops = Self::rows(f).into_iter().enumerate().filter_map(|(i, r)| {
                    #[allow(clippy::cast_precision_loss)]
                    matches!(r, DiffRow::Hunk(_)).then_some(i as f32 * h)
                });
                // A little slack so the hunk at the top counts as "here".
                let here = self.scroll.offset;
                let target = if c == 'n' {
                    tops.into_iter().find(|y| *y > here + 1.0)
                } else {
                    tops.into_iter().rfind(|y| *y < here - 1.0)
                };
                match target {
                    Some(y) => nav::scroll_y(id, y),
                    None => Task::none(),
                }
            }
            _ => return None,
        };
        Some(Moved::stay().with_task(task))
    }

    pub fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message {
            Message::ToggleLine(key) => {
                if !self.selection.remove(&key) {
                    self.selection.insert(key);
                }
                Update::none()
            }
            Message::ClearSelection => {
                self.selection.clear();
                Update::none()
            }
            Message::Hunk { hunk, action } => {
                let patches = self.patches(std::iter::once((self.file, hunk, None)), action);
                if patches.is_empty() {
                    return Update::none();
                }
                Update::output(Output::Apply {
                    patches,
                    action,
                    description: format!("{} 1 hunk", action.label()),
                })
            }
            Message::Selected(action) => {
                let count = self.selection.len();
                let patches = self.patches(self.selection_by_hunk().into_iter(), action);
                if patches.is_empty() {
                    return Update::none();
                }
                self.selection.clear();
                Update::output(Output::Apply {
                    patches,
                    action,
                    description: format!(
                        "{} {count} line{}",
                        action.label(),
                        if count == 1 { "" } else { "s" }
                    ),
                })
            }
            Message::SelectFile(i) => {
                if i >= self.diff.files.len() || i == self.file {
                    return Update::none();
                }
                self.file = i;
                self.selection.clear();
                self.scroll = ScrollPos::default();
                if self.spans.contains_key(&i) {
                    Update::none()
                } else {
                    Update::output(Output::Highlight(i, Box::new(self.diff.files[i].clone())))
                }
            }
            Message::Scrolled(pos) => {
                self.scroll = pos;
                Update::none()
            }
            Message::FilesScrolled(pos) => {
                self.files_scroll = pos;
                Update::none()
            }
            Message::ToggleWhitespace => {
                self.ignore_whitespace = !self.ignore_whitespace;
                Update::output(Output::Reload)
            }
            Message::Close => {
                self.close();
                Update::none()
            }
            Message::Blame | Message::FileHistory => {
                let Some(path) = self.current_path() else {
                    return Update::none();
                };
                if matches!(message, Message::FileHistory) {
                    return Update::output(Output::FileHistory(path));
                }
                let rev = match &self.source {
                    Some(DiffSource::Commit(oid)) => Some(oid.to_string()),
                    Some(DiffSource::Stash { id, .. }) => Some(id.to_string()),
                    Some(DiffSource::Range { to, .. }) => Some(to.to_string()),
                    _ => None,
                };
                Update::output(Output::Blame { path, rev })
            }
        }
    }

    /// Changed files of the commit/stash being shown.
    pub fn files_view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let accent = self.role.accent(t);
        let files = &self.diff.files;
        let title = if files.is_empty() {
            "FILES".to_owned()
        } else {
            format!("FILES ({})", files.len())
        };
        let body: Element<'a, Message> = if self.source.is_none() {
            widgets::empty_hint(t, "pick a commit ↑")
        } else if self.loading && files.is_empty() {
            widgets::empty_hint(t, "loading…")
        } else if files.is_empty() {
            widgets::empty_hint(t, "no file changes")
        } else {
            let path_of = |f: &FileDiff| {
                f.display_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            };
            let counts_of = |f: &FileDiff| -> widgets::EntryCounts {
                let (added, removed) = f.line_stats();
                Some((!f.is_binary).then(|| gf_git::LineCounts {
                    added: u32::try_from(added).unwrap_or(u32::MAX),
                    removed: u32::try_from(removed).unwrap_or(u32::MAX),
                }))
            };
            let cols = files
                .iter()
                .map(|f| {
                    widgets::file_entry_columns(
                        f.display_path()
                            .map(PathBuf::as_path)
                            .unwrap_or(std::path::Path::new("")),
                        counts_of(f),
                    )
                })
                .max()
                .unwrap_or(0);
            let content_width = t.cols(cols) + 30.0;
            let current = self.file;
            virtual_list(
                FILES_ID,
                files.len(),
                Self::files_row_height(t),
                content_width,
                self.files_scroll,
                move |i| {
                    let f = &files[i];
                    let code = match f.change {
                        FileChange::Added => "A",
                        FileChange::Deleted => "D",
                        FileChange::Modified => "M",
                        FileChange::Renamed { .. } => "R",
                        FileChange::Copied { .. } => "C",
                    };
                    let color = widgets::change_color(t, code.chars().next().unwrap_or(' '));
                    let path = f
                        .display_path()
                        .map(PathBuf::as_path)
                        .unwrap_or(std::path::Path::new(""));
                    let line = widgets::file_entry(t, code, color, path, counts_of(f));
                    widgets::tip(
                        t,
                        widgets::list_row(t, line, i == current, Message::SelectFile(i)),
                        path_of(f),
                    )
                },
                Message::FilesScrolled,
            )
        };
        widgets::accent_panel(t, title, accent, body)
    }

    pub fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let accent = self.role.accent(t);
        let Some(source) = &self.source else {
            let hint = match self.role {
                Role::Local => "select a file in the working tree to see its changes",
                Role::Commit => {
                    "select a commit in the history (or a stash) to see what it changed"
                }
            };
            return widgets::accent_panel(
                t,
                self.role.title().into(),
                accent,
                widgets::empty_hint(t, hint),
            );
        };

        let files = &self.diff.files;
        let mut title = source.title();
        if self.role == Role::Commit {
            if let Some(f) = files.get(self.file) {
                let path = f
                    .display_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                title = format!("{title} · {path}");
            }
        }
        let mut header = row![mono(t, title).wrapping(Wrapping::None).width(Length::Fill)]
            .spacing(4)
            .align_y(iced::Alignment::Center);
        if files.len() > 1 {
            header = header
                .push(dim(t, format!("{}/{}", self.file + 1, files.len())))
                .push(widgets::icon_button(
                    t,
                    "◂",
                    t.accent(),
                    "previous file",
                    Message::SelectFile(self.file.saturating_sub(1)),
                ))
                .push(widgets::icon_button(
                    t,
                    "▸",
                    t.accent(),
                    "next file",
                    Message::SelectFile(self.file + 1),
                ));
        }
        if self.current_path().is_some() {
            header = header
                .push(widgets::tip(
                    t,
                    widgets::text_button(t, "blame", None, Some(Message::Blame)),
                    "who last changed each line of this file".into(),
                ))
                .push(widgets::tip(
                    t,
                    widgets::text_button(t, "file history", None, Some(Message::FileHistory)),
                    "commits that touched this file (follows renames)".into(),
                ));
        }
        header = header
            .push(widgets::tip(
                t,
                widgets::text_button(
                    t,
                    if self.ignore_whitespace {
                        "␣ ignored"
                    } else {
                        "␣ shown"
                    },
                    None,
                    Some(Message::ToggleWhitespace),
                ),
                "whitespace-only changes: shown / ignored".into(),
            ))
            .push(widgets::icon_button(t, "✕", t.dim, "close", Message::Close));

        let mut toolbar = column![header];
        if !self.selection.is_empty() {
            let mut sel = row![colored(
                t,
                format!("{} selected", self.selection.len()),
                t.accent()
            )]
            .spacing(4)
            .align_y(iced::Alignment::Center);
            for &a in source.actions() {
                let label = format!("{} lines", a.label());
                sel = sel.push(if a == Action::Discard {
                    widgets::danger_button(t, &label, Some(Message::Selected(a)))
                } else {
                    widgets::text_button(t, &label, None, Some(Message::Selected(a)))
                });
            }
            sel = sel.push(widgets::text_button(
                t,
                "clear",
                None,
                Some(Message::ClearSelection),
            ));
            toolbar = toolbar.push(sel);
        } else if !source.actions().is_empty() && !files.is_empty() {
            toolbar = toolbar.push(dim(
                t,
                "click lines to select them, then stage/discard just those",
            ));
        }

        let body: Element<'a, Message> = if let Some(e) = &self.error {
            colored(t, format!("  ⚠ {e}"), t.error()).into()
        } else if self.loading && files.is_empty() {
            widgets::empty_hint(t, "loading diff…")
        } else if let Some(f) = files.get(self.file) {
            let rows = Self::rows(f);
            // two line-number columns, marker, then the widest line.
            let line_cols = self.widths.get(self.file).copied().unwrap_or(0);
            let content_width = t.cols(13 + line_cols + 2);
            virtual_list(
                self.role.list_id(),
                rows.len(),
                Self::row_height(t),
                content_width,
                self.scroll,
                move |i| self.row_view(t, f, rows[i]),
                Message::Scrolled,
            )
        } else {
            let hint = match source {
                DiffSource::WorkTree(_) => "no unstaged changes left in this file",
                DiffSource::Staged(_) => "nothing staged for this file",
                _ => "no changes",
            };
            widgets::empty_hint(t, hint)
        };

        widgets::accent_panel(
            t,
            self.role.title().into(),
            accent,
            column![toolbar, body].spacing(4),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gf_git::cli::parse::diff::parse_diff;

    const DIFF: &str = "\
diff --git a/f.txt b/f.txt
index 1111111..2222222 100644
--- a/f.txt
+++ b/f.txt
@@ -1,3 +1,3 @@
 a
-b
+B
 c
diff --git a/g.txt b/g.txt
index 1111111..2222222 100644
--- a/g.txt
+++ b/g.txt
@@ -1 +1 @@
-x
+y
";

    fn state(source: DiffSource) -> State {
        let mut s = State::new(source.role());
        s.show(source.clone());
        let files = parse_diff(DIFF.as_bytes()).expect("parse");
        s.loaded(
            &source,
            Ok(Arc::new(LoadedDiff {
                files,
                first_spans: None,
            })),
        );
        s
    }

    #[test]
    fn hunk_action_builds_a_patch() {
        let mut s = state(DiffSource::WorkTree("f.txt".into()));
        let out = s
            .update(Message::Hunk {
                hunk: 0,
                action: Action::Stage,
            })
            .output;
        assert!(
            matches!(out, Some(Output::Apply { patches, action: Action::Stage, .. }) if patches.len() == 1)
        );
    }

    #[test]
    fn line_selection_builds_a_partial_patch_and_clears() {
        let mut s = state(DiffSource::WorkTree("f.txt".into()));
        let _ = s.update(Message::ToggleLine((0, 0, 2)));
        match s.update(Message::Selected(Action::Stage)).output {
            Some(Output::Apply {
                patches,
                description,
                ..
            }) => {
                assert_eq!(description, "stage 1 line");
                assert!(patches[0].contains(" b\n+B\n"), "{}", patches[0]);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(s.selection.is_empty());
    }

    #[test]
    fn selecting_another_file_requests_highlighting_once() {
        let oid: Oid = "a".repeat(40).parse().expect("oid");
        let mut s = state(DiffSource::Commit(oid));
        assert_eq!(s.role, Role::Commit);
        assert!(matches!(
            s.update(Message::SelectFile(1)).output,
            Some(Output::Highlight(1, _))
        ));
        s.highlighted(&s.source.clone().expect("source"), 1, Arc::default());
        let _ = s.update(Message::SelectFile(0));
        assert!(
            s.update(Message::SelectFile(1)).output.is_none(),
            "already highlighted"
        );
        assert!(
            s.update(Message::SelectFile(9)).output.is_none(),
            "out of range"
        );
    }

    #[test]
    fn rows_flatten_the_selected_file() {
        let files = parse_diff(DIFF.as_bytes()).expect("parse");
        let rows = State::rows(&files[0]);
        // header + hunk header + 4 lines
        assert_eq!(rows.len(), 6);
    }

    #[test]
    fn sources_pick_their_view() {
        assert_eq!(DiffSource::Staged("a".into()).role(), Role::Local);
        assert_eq!(
            DiffSource::Stash {
                index: 0,
                id: "b".repeat(40).parse().expect("oid")
            }
            .role(),
            Role::Commit
        );
    }

    #[test]
    fn stale_results_are_ignored() {
        let mut s = State::new(Role::Local);
        s.show(DiffSource::WorkTree("a".into()));
        s.show(DiffSource::WorkTree("b".into()));
        s.loaded(&DiffSource::WorkTree("a".into()), Ok(Arc::default()));
        assert!(s.loading);
    }
}
