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

mod files;
mod loading;
mod select;
pub use loading::Completion;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gf_core::model::{DiffLine, FileChange, FileDiff, LineKind, Oid};
use gf_git::{DiffTarget, FileVersion, patch};
use iced::advanced::mouse::{Button, Click, click};
use iced::widget::text::Wrapping;
use iced::widget::{Space, button, column, container, row};
use iced::{Color, Element, Length, Padding, Task};

use crate::feature::{Cx, Update};
use crate::features::context_menu::{Entry, Menu};
use crate::highlight::{self, Spans};
use crate::nav::{self, Key, Moved, Nav};
use crate::theme::{Tokens, style};
use crate::widgets::file_tree;
use crate::widgets::memo::{self, Memo};
use crate::widgets::selectable_line::TextPointer;
use crate::widgets::virtual_list::{ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim, mono};

/// Scrollable id of the commit's file list.
const FILES_ID: &str = "gf-commit-files";
/// Files larger than this (in diff lines) are shown without highlighting.
const MAX_HIGHLIGHT_LINES: usize = 20_000;
/// A file's diff past this many lines or bytes waits for the user to ask
/// for it ([`Message::ShowLarge`]) instead of drawing at once. (Past
/// [`gf_git::DIFF_LIMIT`] it is not read at all.)
const LARGE_DIFF_LINES: usize = MAX_HIGHLIGHT_LINES;
const LARGE_DIFF_BYTES: usize = 1024 * 1024;

/// How big one file's diff is: lines and bytes of their text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffSize {
    pub lines: usize,
    pub bytes: usize,
}

impl DiffSize {
    fn of(f: &FileDiff) -> Self {
        let lines = f.hunks.iter().flat_map(|h| &h.lines);
        Self {
            lines: lines.clone().count(),
            bytes: lines.map(|l| l.content.len()).sum(),
        }
    }

    fn is_large(self) -> bool {
        self.lines > LARGE_DIFF_LINES || self.bytes > LARGE_DIFF_BYTES
    }

    /// `300,000 lines · 10.0 MiB`
    fn describe(self) -> String {
        let mut digits = self.lines.to_string();
        let mut grouped = String::new();
        while digits.len() > 3 {
            let tail = digits.split_off(digits.len() - 3);
            grouped = format!(",{tail}{grouped}");
        }
        #[allow(clippy::cast_precision_loss)]
        let mib = self.bytes as f64 / (1024.0 * 1024.0);
        let plural = if self.lines == 1 { "" } else { "s" };
        format!("{digits}{grouped} line{plural} · {mib:.1} MiB")
    }
}

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

/// The line under a diff's header holds the selection buttons when needed.
/// Always the same height, so selecting the first
/// line of a drag does not push the code down under the mouse.
fn bar_line<'a>(t: &Tokens, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .height(Length::Fixed((t.font_size * 1.9).round()))
        .align_y(iced::Alignment::Center)
        .into()
}

/// Local titles show the repository-relative path. Historical files show
/// the name and a dimmed folder suffix, with a label for stashes and ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Title {
    lead: String,
    name: String,
    rest: String,
}

impl Title {
    fn of(source: &DiffSource, path: Option<&std::path::Path>) -> Self {
        let label = source.label();
        let Some(path) = path.filter(|_| source.role() == Role::Commit) else {
            return Self {
                lead: label,
                name: String::new(),
                rest: String::new(),
            };
        };
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into(),
        );
        let dir = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map(|d| format!("  {}/", d.display()))
            .unwrap_or_default();
        Self {
            lead: if label.is_empty() {
                label
            } else {
                format!("{label} · ")
            },
            name,
            rest: dir,
        }
    }

    /// The whole title on one line (the hover text).
    fn full(&self) -> String {
        format!("{}{}{}", self.lead, self.name, self.rest)
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
    /// Everything that differs between the checked-out commit (`from`) and
    /// another branch's tip (`to`); `base`/`other` are the names to show.
    Compare {
        base: String,
        other: String,
        from: Oid,
        to: Oid,
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
            Self::Commit(_) | Self::Stash { .. } | Self::Range { .. } | Self::Compare { .. } => {
                Role::Commit
            }
        }
    }

    fn end_revision(&self) -> Option<&Oid> {
        match self {
            Self::Commit(oid)
            | Self::Stash { id: oid, .. }
            | Self::Range { to: oid, .. }
            | Self::Compare { to: oid, .. } => Some(oid),
            _ => None,
        }
    }

    /// Revision of the side that contains the file.
    fn file_revision(&self, file: &FileDiff) -> Option<String> {
        let (base, target, _) = self.comparison_versions();
        let version = if file.new_path.is_some() {
            target
        } else {
            base
        };
        match version {
            FileVersion::Revision(rev) => Some(rev),
            _ => None,
        }
    }

    /// Versions and pane labels matching the diff's Git target.
    pub fn comparison_versions(&self) -> (FileVersion, FileVersion, [&'static str; 2]) {
        use FileVersion::{Index, Revision, WorkTree};
        match self {
            Self::WorkTree(_) | Self::Untracked(_) => (Index, WorkTree, ["Index", "Working tree"]),
            Self::Staged(_) => (Revision("HEAD".into()), Index, ["HEAD", "Index"]),
            Self::Commit(oid) => (
                Revision(format!("{oid}^1")),
                Revision(oid.to_string()),
                ["Parent", "Commit"],
            ),
            Self::Stash { id, .. } => (
                Revision(format!("{id}^1")),
                Revision(id.to_string()),
                ["Base", "Stash"],
            ),
            Self::Range { from, to, .. } => (
                Revision(from.clone()),
                Revision(to.to_string()),
                ["From", "To"],
            ),
            Self::Compare { from, to, .. } => (
                Revision(from.to_string()),
                Revision(to.to_string()),
                ["Current", "Branch"],
            ),
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
            Self::Compare { from, to, .. } => (
                DiffTarget::Range {
                    from: from.to_string(),
                    to: to.to_string(),
                },
                Vec::new(),
            ),
        }
    }

    /// Label for sources that need context in the diff header.
    fn label(&self) -> String {
        match self {
            Self::WorkTree(p) | Self::Staged(p) | Self::Untracked(p) => p.display().to_string(),
            Self::Commit(_) => String::new(),
            Self::Stash { index, .. } => format!("stash@{{{index}}}"),
            Self::Compare { base, other, .. } => format!("{base} → {other}"),
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

/// A loaded diff, with what the view needs from walking all of it
/// computed off the UI thread (see [`LoadedDiff::new`]).
#[derive(Debug, Default)]
pub struct LoadedDiff {
    pub files: Vec<FileDiff>,
    /// Highlighting of file 0 (the others are highlighted when selected).
    pub first_spans: Option<Arc<FileSpans>>,
    /// Widest line of each file in monospace columns (for sideways scroll).
    pub widths: Vec<usize>,
    /// Size of each file's diff (large ones wait to be asked for).
    sizes: Vec<DiffSize>,
    line_counts: Vec<select::LineCounts>,
    entry_counts: Vec<widgets::EntryCounts>,
    file_list_columns: usize,
    file_tree: file_tree::Tree,
    file_tree_columns: usize,
    submodules: Vec<bool>,
}

impl LoadedDiff {
    /// Walks every line: call it off the UI thread. `highlight` highlights
    /// the first file.
    pub fn new(files: Vec<FileDiff>, highlight: bool) -> Self {
        let first_spans = files
            .first()
            .filter(|_| highlight)
            .map(|f| Arc::new(highlight_file(f)));
        let widths = files
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
        let sizes = files.iter().map(DiffSize::of).collect();
        let line_counts = files.iter().map(select::LineCounts::new).collect();
        let entry_counts: Vec<_> = files
            .iter()
            .map(|f| {
                Some((!f.is_binary).then(|| {
                    let (added, removed) = f.line_stats();
                    gf_git::LineCounts {
                        added: u32::try_from(added).unwrap_or(u32::MAX),
                        removed: u32::try_from(removed).unwrap_or(u32::MAX),
                    }
                }))
            })
            .collect();
        let (file_list_columns, file_tree_columns) =
            files
                .iter()
                .zip(&entry_counts)
                .fold((0, 0), |(list, tree), (file, &counts)| {
                    let path = files::display_path(file);
                    let width = widgets::file_entry_columns(path, counts);
                    let old = files::original_name(file)
                        .map_or(0, |name| columns(&name.to_string_lossy()) + 3);
                    (list.max(width), tree.max(width + old + 10))
                });
        let file_tree = file_tree::Tree::new(
            files
                .iter()
                .enumerate()
                .map(|(i, file)| (i, files::display_path(file))),
        );
        let submodules = files.iter().map(FileDiff::is_submodule).collect();
        Self {
            files,
            first_spans,
            widths,
            sizes,
            line_counts,
            entry_counts,
            file_list_columns,
            file_tree,
            file_tree_columns,
            submodules,
        }
    }
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

use select::{LineKey, Pos, Range};

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
    generation: u64,
    source: Option<DiffSource>,
    diff: Arc<LoadedDiff>,
    /// Highlighting of files other than the first, loaded on selection.
    spans: HashMap<usize, Arc<FileSpans>>,
    file: usize,
    loading: bool,
    error: Option<String>,
    /// Selected text, as in a text viewer: copied with ⌘C; the changed
    /// lines it touches can be staged / unstaged. Several with ⌘.
    ranges: Vec<Range>,
    /// The mouse button is down after a press on a line: moving extends
    /// the last range.
    dragging: bool,
    last_click: Option<(LineKey, Click)>,
    /// For a double/triple-click drag, keep the original whole word/line.
    drag_unit: Option<(click::Kind, Range)>,
    /// Where the mouse last went down: where ⇧-click extends from.
    caret: Option<Pos>,
    ignore_whitespace: bool,
    scroll: ScrollPos,
    files_scroll: ScrollPos,
    folders: file_tree::Folders<()>,
    visible_files: Memo<(memo::Same<LoadedDiff>, u64), Rc<[usize]>>,
    /// Rows of the file shown, per loaded diff and file.
    /// The rows index into the diff's hunks: keyed by the diff itself.
    rows: Memo<(memo::Same<LoadedDiff>, usize), Rc<[DiffRow]>>,
    /// Large files the user asked to see (by path, so a reload of the same
    /// source keeps them shown). Cleared when the source changes.
    shown_large: BTreeSet<PathBuf>,
    /// File to show first once the loading diff arrives (Find File).
    preferred: Option<std::path::PathBuf>,
    /// The files list should scroll to the file picked by [`Self::prefer_file`].
    reveal_pending: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Raw pointer press, before applying the app's keyboard modifiers.
    LineClicked(LineKey, TextPointer),
    /// ⌘ + mouse down: another selection starts there, keeping the others.
    LinePressedAdd(Pos),
    /// ⇧ + mouse down: the selection now runs from where it started (or
    /// the last press) to here.
    LinePressedRange(Pos),
    /// The mouse moved to `Pos` while the button is down.
    LineDragged(LineKey, TextPointer),
    /// Mouse up, or the mouse left the diff.
    DragEnd,
    /// Draw the shown file although its diff is large (⏎).
    ShowLarge,
    /// ⌘A: all the text of the shown file.
    SelectAll,
    /// Copy the selected diff text.
    Copy,
    ClearSelection,
    Hunk {
        hunk: usize,
        action: Action,
    },
    /// Put the shown submodule back on its recorded commit.
    DiscardSubmodule,
    Selected(Action),
    SelectFile(usize),
    FocusFolder(PathBuf),
    ToggleFolder(PathBuf),
    SetTree(bool),
    FileMenu(usize),
    OpenFileMenu,
    FileAction(FileAction),
    Scrolled(ScrollPos),
    FilesScrolled(ScrollPos),
    ToggleWhitespace,
    Close,
    ViewChanges,
}

#[derive(Debug, Clone)]
pub enum FileAction {
    View(PathBuf, FileVersion),
    Blame(PathBuf, String),
    History(PathBuf, Oid),
    CopyPath(PathBuf),
    Reveal(PathBuf),
}

#[derive(Debug, Clone)]
pub enum Output {
    SetTree(bool),
    Menu(Menu<Message>),
    FileMenu {
        menu: Menu<Message>,
        highlight: Option<usize>,
    },
    FileAction(FileAction),
    /// Show the selected file with the currently displayed changes.
    ViewChanges {
        source: DiffSource,
        path: PathBuf,
        diff: Arc<LoadedDiff>,
        file: usize,
    },
    /// Apply these patches in order. Discards must be confirmed first.
    Apply {
        patches: Vec<String>,
        action: Action,
        description: String,
    },
    /// Discard a submodule's work tree changes (whole entry; the app
    /// confirms first).
    DiscardSubmodule(PathBuf),
    /// Reload the current source (options changed).
    Reload,
    /// Highlight file n of the currently displayed diff.
    Highlight(usize),
    /// Put `text` on the clipboard; `what` says what it is (`3 lines`).
    Copy {
        text: String,
        what: String,
    },
}

impl State {
    pub fn new(role: Role) -> Self {
        Self {
            role,
            generation: 0,
            source: None,
            diff: Arc::default(),
            spans: HashMap::new(),
            file: 0,
            loading: false,
            error: None,
            ranges: Vec::new(),
            dragging: false,
            last_click: None,
            drag_unit: None,
            caret: None,
            ignore_whitespace: false,
            scroll: ScrollPos::default(),
            files_scroll: ScrollPos::default(),
            folders: file_tree::Folders::default(),
            visible_files: Memo::default(),
            rows: Memo::default(),
            shown_large: BTreeSet::new(),
            preferred: None,
            reveal_pending: false,
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
                        DiffSource::Commit(_)
                            | DiffSource::Stash { .. }
                            | DiffSource::Range { .. }
                            | DiffSource::Compare { .. }
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

    fn can_view_changes(&self) -> bool {
        !self.loading && self.error.is_none() && self.diff.submodules.get(self.file) != Some(&true)
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

    fn clear_selection(&mut self) {
        self.ranges.clear();
        self.dragging = false;
        self.drag_unit = None;
        self.last_click = None;
        self.caret = None;
    }

    pub fn has_selection(&self) -> bool {
        self.ranges.iter().any(|r| !r.is_empty())
    }

    /// Lines with selected text (or whose line break is selected).
    fn selected_lines(&self) -> BTreeSet<LineKey> {
        let mut out = BTreeSet::new();
        for r in &self.ranges {
            let ((a, _), (b, _)) = r.ordered();
            for key in self.lines_between(a, b) {
                let len = self.line_at(key).map_or(1, |l| select::len(&l.content));
                if r.span(key, len).is_some() {
                    out.insert(key);
                }
            }
        }
        out
    }

    /// Selected lines that are changes (what stage / unstage act on):
    /// context lines are selected for copying only.
    fn selected_changes(&self) -> Vec<LineKey> {
        self.selected_lines()
            .into_iter()
            .filter(|&k| self.line_at(k).is_some_and(|l| l.kind != LineKind::Context))
            .collect()
    }

    fn line_at(&self, (f, h, l): LineKey) -> Option<&DiffLine> {
        self.diff.files.get(f)?.hunks.get(h)?.lines.get(l)
    }

    /// Lines of the shown file from `a` to `b` (either order), in the
    /// order they are shown.
    fn lines_between(&self, a: LineKey, b: LineKey) -> Vec<LineKey> {
        let Some(f) = self.diff.files.get(self.file) else {
            return Vec::new();
        };
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        (a.1..=b.1)
            .flat_map(|h| {
                let start = if h == a.1 { a.2 } else { 0 };
                let end = if h == b.1 {
                    b.2 + 1
                } else {
                    f.hunks[h].lines.len()
                };
                (start..end).map(move |l| (self.file, h, l))
            })
            .collect()
    }

    /// Exactly the selected text, as shown (`+` / `-` markers included,
    /// line numbers not); `None` when nothing is selected.
    pub fn selected_text(&self) -> Option<(String, String)> {
        let mut ranges: Vec<&Range> = self.ranges.iter().filter(|r| !r.is_empty()).collect();
        if ranges.is_empty() {
            return None;
        }
        ranges.sort_by_key(|r| r.ordered().0);
        let mut text = String::new();
        for r in ranges {
            let ((a, _), (b, _)) = r.ordered();
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&select::range_text(
                r,
                self.lines_between(a, b)
                    .into_iter()
                    .filter_map(|k| self.line_at(k).map(|l| (k, l.kind, l.content.as_str()))),
            ));
        }
        let lines = self.selected_lines().len();
        let what = match (lines, text.chars().count()) {
            (1, 1) => "1 character".to_owned(),
            (1, n) => format!("{n} characters"),
            (n, _) => format!("{n} lines"),
        };
        Some((text, what))
    }

    fn selection_by_hunk(&self) -> Vec<(usize, usize, Option<BTreeSet<usize>>)> {
        let mut out: Vec<(usize, usize, Option<BTreeSet<usize>>)> = Vec::new();
        for (f, h, l) in self.selected_changes() {
            match out.last_mut() {
                Some((lf, lh, Some(lines))) if *lf == f && *lh == h => {
                    lines.insert(l);
                }
                _ => out.push((f, h, Some(BTreeSet::from([l])))),
            }
        }
        out
    }

    /// Path of the shown file (its new path, else its old one).
    fn current_file_path(&self) -> Option<PathBuf> {
        self.diff.files.get(self.file)?.display_path().cloned()
    }

    /// Size of the shown file's diff when it is large and the user has not
    /// asked to see it yet: the view shows a notice instead of its lines.
    fn held_back(&self) -> Option<DiffSize> {
        let size = *self.diff.sizes.get(self.file)?;
        let path = self.current_file_path()?;
        (size.is_large() && !self.shown_large.contains(&path)).then_some(size)
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
                if !partial && f.is_submodule() && actions.contains(&Action::Discard) {
                    bar = bar.push(widgets::tip(
                        t,
                        widgets::danger_button(
                            t,
                            "discard submodule",
                            Some(Message::DiscardSubmodule),
                        ),
                        "check out the commit this repository records for the submodule".into(),
                    ));
                }
                if partial {
                    for &a in actions {
                        let msg = Message::Hunk {
                            hunk: hi,
                            action: a,
                        };
                        let label = a.label();
                        bar = bar.push(if a == Action::Discard {
                            widgets::danger_button(t, label, Some(msg))
                        } else {
                            widgets::text_button(t, label, None, Some(msg))
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
                        colored(t, widgets::format::code_line(&line.content), color)
                            .wrapping(Wrapping::None)
                            .into()
                    }
                };
                // The gutter has fixed widths, so the marker and the code
                // start where the selectable line expects them.
                let cell = |s: Element<'a, Message>, cols: usize| {
                    container(s).width(Length::Fixed(t.cols(cols))).clip(true)
                };
                let mut text_row = row![
                    cell(dim(t, num(line.old_lineno)).into(), 5),
                    cell(dim(t, num(line.new_lineno)).into(), 5),
                    cell(colored(t, format!(" {marker} "), marker_color).into(), 3),
                    content,
                ]
                .align_y(iced::Alignment::Center);
                if line.no_newline_at_eof {
                    text_row =
                        text_row.push(dim(t, "  ⏎̸ no newline at end").wrapping(Wrapping::None));
                }
                // Text selection as in a text viewer: press, drag, ⌘C.
                let key = (fi, hi, li);
                let len = select::len(&line.content);
                let spans = self
                    .ranges
                    .iter()
                    .filter_map(|r| r.span(key, len))
                    .collect();
                let geometry = widgets::selectable_line::Geometry {
                    marker_x: t.cols(11),
                    code_x: t.cols(13),
                    cell: t.char_width(),
                    size: t.font_size,
                };
                widgets::selectable_line::selectable_line(
                    container(text_row).width(Length::Fill).height(Length::Fill),
                    &line.content,
                    geometry,
                    move |pointer| Message::LineClicked(key, pointer),
                )
                .background(style::diff_line_color(t, line.kind))
                .selected(spans, t.diff_selection, marker)
                .on_drag(self.dragging, move |pointer| {
                    Message::LineDragged(key, pointer)
                })
                .into()
            }
        }
    }

    fn row_height(t: &Tokens) -> f32 {
        (t.font_size * 1.6).round()
    }

    /// Keys for the diff itself: arrows and PgUp/PgDn scroll, Home/End
    /// jump, `n`/`p` go to the next/previous hunk.
    pub fn diff_nav(&self, nav: Nav, t: &Tokens) -> Option<Moved<Message>> {
        if self.held_back().is_some() {
            return match nav.key {
                Key::Enter => Some(Moved::message(Message::ShowLarge)),
                Key::Char('n' | 'p') => Some(Moved::stay()),
                _ => None,
            };
        }
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

    fn click_range(&self, key: LineKey, pointer: TextPointer, kind: click::Kind) -> Range {
        let at = (key, pointer.caret);
        let Some(line) = self.line_at(key) else {
            return Range::caret(at);
        };
        match kind {
            click::Kind::Single => Range::caret(at),
            click::Kind::Double => select::word(&line.content, (key, pointer.hit)),
            click::Kind::Triple => Range {
                anchor: (key, 0),
                head: (key, select::len(&line.content)),
            },
        }
    }

    fn line_clicked(&mut self, key: LineKey, pointer: TextPointer) -> Update<Message, Output> {
        let at = (key, pointer.caret);
        if pointer.button == Button::Right {
            let hit = (key, pointer.hit);
            if !self.ranges.iter().any(|r| {
                let (start, end) = r.ordered();
                start <= hit && hit < end
            }) {
                self.clear_selection();
                self.caret = Some(at);
            }
            self.last_click = None;
            let mut entries = Vec::new();
            if self.has_selection() {
                entries.push(Entry::item('c', "Copy", "⌘C", Message::Copy));
            }
            entries.push(Entry::item('a', "Select All", "⌘A", Message::SelectAll));
            return Update::output(Output::Menu(Menu {
                title: "Diff".into(),
                entries,
            }));
        }

        let previous = self
            .last_click
            .filter(|(last, _)| *last == key)
            .map(|(_, click)| click);
        let click = Click::new(pointer.point, pointer.button, previous);
        self.last_click = Some((key, click));
        let range = self.click_range(key, pointer, click.kind());
        self.ranges = vec![range];
        self.caret = Some(range.anchor);
        self.dragging = true;
        self.drag_unit = (click.kind() != click::Kind::Single).then_some((click.kind(), range));
        Update::none()
    }

    fn historical_preview(&self, i: usize) -> Option<(&PathBuf, String)> {
        let file = self
            .diff
            .files
            .get(i)
            .filter(|_| !self.diff.submodules[i])?;
        let revision = self.source.as_ref()?.file_revision(file)?;
        Some((file.display_path()?, revision))
    }

    fn file_menu(&self, i: usize) -> Option<Menu<Message>> {
        let file = self.diff.files.get(i)?;
        let path = file.display_path()?;
        let source = self.source.as_ref()?;
        let end = source.end_revision()?;
        let title = path.display().to_string();
        let mut entries = Vec::new();
        if let Some((path, revision)) = self.historical_preview(i) {
            entries.push(Entry::item(
                'v',
                "view file",
                "show the entire file at this revision",
                Message::FileAction(FileAction::View(
                    path.clone(),
                    FileVersion::Revision(revision.clone()),
                )),
            ));
            if !file.is_binary {
                entries.push(Entry::item(
                    'b',
                    "blame",
                    "who changed each line at this revision",
                    Message::FileAction(FileAction::Blame(path.clone(), revision)),
                ));
            }
        }
        entries.extend([
            Entry::item(
                'h',
                "file history",
                format!("git log --follow {} -- {title}", end.short()),
                Message::FileAction(FileAction::History(path.clone(), end.clone())),
            ),
            Entry::item(
                'y',
                "copy path",
                title.clone(),
                Message::FileAction(FileAction::CopyPath(path.clone())),
            ),
            Entry::item(
                'f',
                format!("reveal in {}", crate::platform::FILE_MANAGER),
                "show the path in the working tree",
                Message::FileAction(FileAction::Reveal(path.clone())),
            ),
        ]);
        Some(Menu { title, entries })
    }

    pub fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message {
            Message::LineClicked(key, pointer) => self.line_clicked(key, pointer),
            Message::LinePressedAdd(at) => {
                self.last_click = None;
                self.drag_unit = None;
                self.ranges.push(Range::caret(at));
                self.caret = Some(at);
                self.dragging = true;
                Update::none()
            }
            Message::LinePressedRange(at) => {
                self.last_click = None;
                self.drag_unit = None;
                let anchor = self
                    .ranges
                    .last()
                    .map(|r| r.anchor)
                    .or(self.caret)
                    .filter(|((f, _, _), _)| *f == self.file)
                    .unwrap_or(at);
                self.ranges.pop();
                self.ranges.push(Range { anchor, head: at });
                self.dragging = true;
                Update::none()
            }
            Message::LineDragged(key, pointer) => {
                if !self.dragging {
                    return Update::none();
                }
                let unit = self.drag_unit.map(|(kind, start)| {
                    let next = self.click_range(key, pointer, kind);
                    if next.anchor < start.anchor {
                        Range {
                            anchor: start.head,
                            head: next.anchor,
                        }
                    } else {
                        Range {
                            anchor: start.anchor,
                            head: next.head.max(start.head),
                        }
                    }
                });
                if let Some(r) = self.ranges.last_mut() {
                    let next = unit.unwrap_or(Range {
                        anchor: r.anchor,
                        head: (key, pointer.caret),
                    });
                    if *r != next {
                        *r = next;
                        self.last_click = None;
                    }
                }
                Update::none()
            }
            Message::DragEnd => {
                self.dragging = false;
                self.drag_unit = None;
                // A click without a drag only places the caret.
                self.ranges.retain(|r| !r.is_empty());
                Update::none()
            }
            Message::ShowLarge => {
                if let Some(path) = self.held_back().and(self.current_file_path()) {
                    self.shown_large.insert(path);
                }
                Update::none()
            }
            // Nothing of a held-back file is drawn to select.
            Message::SelectAll if self.held_back().is_some() => Update::none(),
            Message::SelectAll => {
                self.clear_selection();
                if let Some(f) = self.diff.files.get(self.file) {
                    let fi = self.file;
                    let first = f.hunks.iter().position(|h| !h.lines.is_empty());
                    let last = f.hunks.iter().rposition(|h| !h.lines.is_empty());
                    if let (Some(a), Some(b)) = (first, last) {
                        let end = f.hunks[b].lines.len() - 1;
                        let len = select::len(&f.hunks[b].lines[end].content);
                        self.ranges = vec![Range {
                            anchor: ((fi, a, 0), 0),
                            head: ((fi, b, end), len),
                        }];
                    }
                }
                Update::none()
            }
            Message::Copy => match self.selected_text() {
                Some((text, what)) => Update::output(Output::Copy { text, what }),
                None => Update::none(),
            },
            Message::ClearSelection => {
                self.clear_selection();
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
            Message::DiscardSubmodule => match self.current_path() {
                Some(path) => Update::output(Output::DiscardSubmodule(path)),
                None => Update::none(),
            },
            Message::Selected(action) => {
                let count = self.selected_changes().len();
                let patches = self.patches(self.selection_by_hunk().into_iter(), action);
                if patches.is_empty() {
                    return Update::none();
                }
                self.clear_selection();
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
            Message::SetTree(tree) => {
                self.folders.focused = None;
                self.files_scroll = ScrollPos::default();
                Update {
                    task: nav::scroll_y(FILES_ID, 0.0),
                    output: Some(Output::SetTree(tree)),
                }
            }
            Message::FocusFolder(path) => {
                self.folders.focused = Some(((), path));
                Update::none()
            }
            Message::ToggleFolder(path) => {
                self.folders.toggle((), path);
                Update::none()
            }
            Message::SelectFile(i) => {
                self.folders.focused = None;
                if i >= self.diff.files.len() || i == self.file {
                    return Update::none();
                }
                self.file = i;
                self.clear_selection();
                self.scroll = ScrollPos::default();
                if self.spans.contains_key(&i) {
                    Update::none()
                } else {
                    Update::output(Output::Highlight(i))
                }
            }
            Message::FileMenu(i) => {
                let Some(menu) = self.file_menu(i) else {
                    return Update::none();
                };
                let highlight = match self.update(Message::SelectFile(i)).output {
                    Some(Output::Highlight(i)) => Some(i),
                    _ => None,
                };
                Update::output(Output::FileMenu { menu, highlight })
            }
            Message::OpenFileMenu => self.update(Message::FileMenu(self.file)),
            Message::FileAction(action) => Update::output(Output::FileAction(action)),
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
            Message::ViewChanges => {
                let (Some(source), Some(path)) = (&self.source, self.current_path()) else {
                    return Update::none();
                };
                if !self.can_view_changes() {
                    return Update::none();
                }
                Update::output(Output::ViewChanges {
                    source: source.clone(),
                    path,
                    diff: self.diff.clone(),
                    file: self.file,
                })
            }
        }
    }

    /// Changed files of the commit/stash being shown.
    /// Stands in for a large diff until the user asks for it.
    fn large_notice<'a>(t: &'a Tokens, size: DiffSize) -> Element<'a, Message> {
        let note = if size.lines > MAX_HIGHLIGHT_LINES {
            "drawing it may be slow, and it is shown without syntax highlighting"
        } else {
            "drawing it may be slow"
        };
        container(
            column![
                colored(t, format!("large diff — {}", size.describe()), t.warning()),
                dim(t, note),
                widgets::tip(
                    t,
                    widgets::text_button(t, "show diff", Some("⏎"), Some(Message::ShowLarge)),
                    "draw this file's diff anyway".into(),
                ),
            ]
            .spacing(4),
        )
        .padding(Padding::from([4, 16]))
        .into()
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
        let shown_path = match source {
            DiffSource::WorkTree(p) | DiffSource::Staged(p) | DiffSource::Untracked(p) => {
                Some(p.clone())
            }
            _ => files.get(self.file).and_then(|f| f.display_path().cloned()),
        };
        let title = Title::of(source, shown_path.as_deref());
        // Clip to leave room for the buttons. Hover shows the full title.
        let title_cell = container(
            row![
                mono(t, title.lead.clone()).wrapping(Wrapping::None),
                mono(t, title.name.clone()).wrapping(Wrapping::None),
                dim(t, title.rest.clone()).wrapping(Wrapping::None),
            ]
            .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .clip(true);
        let mut header = row![widgets::tip(t, title_cell, title.full())]
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
        if shown_path.is_some() && self.diff.submodules.get(self.file) != Some(&true) {
            header = header.push(widgets::tip(
                t,
                widgets::text_button(
                    t,
                    "View",
                    None,
                    self.can_view_changes().then_some(Message::ViewChanges),
                ),
                "show the entire file with changes".into(),
            ));
        }
        let whitespace_color = if self.ignore_whitespace {
            t.accent()
        } else {
            t.dim
        };
        header = header
            .push(widgets::tip(
                t,
                button(mono(t, "[␣]"))
                    .padding(Padding::from([1, 2]))
                    .style(style::text_button(t, whitespace_color))
                    .on_press(Message::ToggleWhitespace),
                if self.ignore_whitespace {
                    "Show whitespace changes".into()
                } else {
                    "Ignore whitespace changes".into()
                },
            ))
            .push(widgets::icon_button(t, "✕", t.dim, "close", Message::Close));

        let mut toolbar = column![header];
        if self.has_selection() {
            let (selected, changes) = self.diff.line_counts[self.file].selected(&self.ranges);
            let mut sel = row![colored(
                t,
                format!(
                    "{selected} line{} selected",
                    if selected == 1 { "" } else { "s" }
                ),
                t.accent()
            )]
            .spacing(4)
            .align_y(iced::Alignment::Center);
            sel = sel.push(widgets::tip(
                t,
                widgets::text_button(t, "copy", Some("⌘C"), Some(Message::Copy)),
                "copy exactly the selected text, + / − included (line numbers are not)".into(),
            ));
            // Stage / unstage the changed lines the selection touches (no
            // discard here: a selection is for reading and copying, and
            // hunks keep their own discard).
            let partial = files.get(self.file).is_some_and(patch::supports_partial);
            for &a in source
                .actions()
                .iter()
                .filter(|a| **a != Action::Discard && partial && changes > 0)
            {
                let label = format!("{} {changes} changed", a.label());
                sel = sel.push(widgets::text_button(
                    t,
                    &label,
                    None,
                    Some(Message::Selected(a)),
                ));
            }
            sel = sel.push(widgets::text_button(
                t,
                "clear",
                None,
                Some(Message::ClearSelection),
            ));
            toolbar = toolbar.push(bar_line(t, sel));
        } else if !files.is_empty() {
            toolbar = toolbar.push(match self.role {
                Role::Local => bar_line(t, Space::new()),
                Role::Commit => bar_line(
                    t,
                    dim(
                        t,
                        "drag to select · double-click a word · ⇧ extends · ⌘A all",
                    ),
                ),
            });
        }

        let body: Element<'a, Message> = if let Some(e) = &self.error {
            colored(t, format!("  ⚠ {e}"), t.error()).into()
        } else if self.loading && files.is_empty() {
            widgets::empty_hint(t, "loading diff…")
        } else if let Some(size) = self.held_back() {
            Self::large_notice(t, size)
        } else if let Some(f) = files.get(self.file) {
            let key = (memo::Same(self.diff.clone()), self.file);
            let rows = self.rows.get(key, || Self::rows(f).into());
            // two line-number columns, marker, then the widest line.
            let line_cols = self.diff.widths.get(self.file).copied().unwrap_or(0);
            let content_width = t.cols(13 + line_cols + 2);
            // A drag over lines ends when the button is let go anywhere
            // over the diff, or the mouse leaves it.
            iced::widget::mouse_area(virtual_list(
                self.role.list_id(),
                rows.len(),
                Self::row_height(t),
                content_width,
                self.scroll,
                move |i| self.row_view(t, f, rows[i]),
                Message::Scrolled,
            ))
            .on_release(Message::DragEnd)
            .on_exit(Message::DragEnd)
            .into()
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
        s.show(source);
        let files = parse_diff(DIFF.as_bytes()).expect("parse");
        let _ = s.loaded(s.generation, Ok(Arc::new(LoadedDiff::new(files, false))));
        s
    }

    #[test]
    fn widths_count_wide_characters_and_tabs() {
        let diff =
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-x\n+\t한글\n";
        let files = parse_diff(diff.as_bytes()).expect("parse");
        // A tab counts as 4 columns, each Hangul syllable as 2.
        assert_eq!(LoadedDiff::new(files, false).widths, [8]);
        assert!(LoadedDiff::new(Vec::new(), true).first_spans.is_none());
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

    /// f.txt's hunk: ` a`, `-b`, `+B`, ` c`.
    fn line(l: usize) -> LineKey {
        (0, 0, l)
    }

    fn pointer(caret: usize, hit: usize) -> TextPointer {
        TextPointer {
            caret,
            hit,
            point: iced::Point::new(40.0, 20.0),
            button: Button::Left,
        }
    }

    fn press((key, caret): Pos) -> Message {
        Message::LineClicked(key, pointer(caret, caret))
    }

    fn drag(s: &mut State, press: Message, to: Pos) {
        let _ = s.update(press);
        let _ = s.update(Message::LineDragged(to.0, pointer(to.1, to.1)));
        let _ = s.update(Message::DragEnd);
    }

    #[test]
    fn repeated_clicks_select_a_word_then_the_line_and_word_drags_expand_by_word() {
        let mut s = state(DiffSource::WorkTree("f.txt".into()));
        let on_b = Message::LineClicked(line(2), pointer(2, 1));
        let _ = s.update(on_b.clone());
        let _ = s.update(Message::DragEnd);
        assert!(!s.has_selection());
        let _ = s.update(on_b.clone());
        let _ = s.update(Message::DragEnd);
        assert_eq!(s.selected_text().map(|t| t.0).as_deref(), Some("B"));
        let _ = s.update(on_b.clone());
        let _ = s.update(Message::DragEnd);
        assert_eq!(s.selected_text().map(|t| t.0).as_deref(), Some("+B"));

        let _ = s.update(Message::ClearSelection);
        let _ = s.update(on_b.clone());
        let _ = s.update(Message::DragEnd);
        drag(&mut s, on_b, (line(1), 1));
        assert_eq!(s.selected_text().map(|t| t.0).as_deref(), Some("b\n+B"));
        assert_eq!(s.diff.line_counts[0].selected(&s.ranges), (2, 2));
    }

    #[test]
    fn context_menu_keeps_a_selection_only_when_opened_on_its_text() {
        let mut s = state(DiffSource::WorkTree("f.txt".into()));
        drag(&mut s, press((line(2), 1)), (line(2), 2));
        let right = TextPointer {
            button: Button::Right,
            ..pointer(2, 1)
        };
        let menu = s.update(Message::LineClicked(line(2), right)).output;
        assert!(matches!(menu, Some(Output::Menu(menu)) if menu.entries.len() == 2));
        assert_eq!(s.selected_text().map(|t| t.0).as_deref(), Some("B"));
        let menu = s.update(Message::LineClicked(line(3), right)).output;
        assert!(matches!(menu, Some(Output::Menu(menu)) if menu.entries.len() == 1));
        assert!(!s.has_selection());
    }

    #[test]
    fn exactly_the_dragged_over_text_is_copied() {
        let oid: Oid = "a".repeat(40).parse().expect("oid");
        let mut s = state(DiffSource::Commit(oid));
        // From the code of ` a` (position 1) to after the `+` of `+B`.
        drag(&mut s, press((line(0), 1)), (line(2), 1));
        assert_eq!(
            s.selected_text(),
            Some(("a\n-b\n+".to_owned(), "3 lines".to_owned()))
        );
        match s.update(Message::Copy).output {
            Some(Output::Copy { text, .. }) => assert_eq!(text, "a\n-b\n+"),
            other => panic!("unexpected {other:?}"),
        }
        // Dragging upwards selects the same way; from the gutter (0) the
        // marker is included.
        drag(&mut s, press((line(3), 2)), (line(1), 0));
        assert_eq!(
            s.selected_text().map(|t| t.0),
            Some("-b\n+B\n c".to_owned())
        );
        // Within one line: just those characters.
        drag(&mut s, press((line(2), 0)), (line(2), 1));
        assert_eq!(
            s.selected_text(),
            Some(("+".to_owned(), "1 character".to_owned()))
        );
    }

    #[test]
    fn a_new_drag_replaces_the_selection_and_modifiers_add_or_extend() {
        let oid: Oid = "a".repeat(40).parse().expect("oid");
        let mut s = state(DiffSource::Commit(oid));
        drag(&mut s, press((line(0), 0)), (line(0), 2));
        // A second plain drag starts over: no leftover block.
        drag(&mut s, press((line(3), 0)), (line(3), 2));
        assert_eq!(s.selected_text().map(|t| t.0), Some(" c".to_owned()));
        // ⌘ keeps what was selected and adds another stretch.
        drag(&mut s, Message::LinePressedAdd((line(0), 0)), (line(0), 2));
        assert_eq!(s.selected_text().map(|t| t.0), Some(" a\n c".to_owned()));
        // ⇧ runs the last stretch from where it started to here.
        let _ = s.update(Message::LinePressedRange((line(1), 2)));
        let _ = s.update(Message::DragEnd);
        assert_eq!(
            s.selected_text().map(|t| t.0),
            Some(" a\n-b\n c".to_owned())
        );
        // A click without a drag selects nothing.
        let _ = s.update(press((line(1), 1)));
        let _ = s.update(Message::DragEnd);
        assert_eq!(s.selected_text(), None);
        // Moving after the button was let go changes nothing.
        let _ = s.update(Message::LineDragged(line(3), pointer(2, 1)));
        assert_eq!(s.selected_text(), None);
    }

    #[test]
    fn copy_requires_a_selection() {
        let oid: Oid = "a".repeat(40).parse().expect("oid");
        let mut s = state(DiffSource::Commit(oid));
        assert!(s.update(Message::Copy).output.is_none());
        let _ = s.update(Message::SelectAll);
        assert_eq!(
            s.selected_text().map(|t| t.0),
            Some(" a\n-b\n+B\n c".to_owned())
        );
        let _ = s.update(Message::SelectFile(1));
        assert_eq!(s.selected_text(), None, "another file starts unselected");
    }

    #[test]
    fn staging_a_selection_takes_the_changed_lines_it_touches() {
        let mut s = state(DiffSource::WorkTree("f.txt".into()));
        // Part of ` a` through part of `+B`: the changed lines are -b, +B.
        drag(&mut s, press((line(0), 1)), (line(2), 1));
        match s.update(Message::Selected(Action::Stage)).output {
            Some(Output::Apply { description, .. }) => {
                assert_eq!(
                    description, "stage 2 lines",
                    "context lines are not counted"
                );
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(s.selected_text(), None, "cleared");
        // Only +B: a partial patch.
        drag(&mut s, press((line(2), 0)), (line(2), 2));
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
    }

    #[test]
    fn selecting_another_file_requests_highlighting_once() {
        let oid: Oid = "a".repeat(40).parse().expect("oid");
        let mut s = state(DiffSource::Commit(oid));
        assert_eq!(s.role, Role::Commit);
        assert!(matches!(
            s.update(Message::SelectFile(1)).output,
            Some(Output::Highlight(1))
        ));
        let _ = s.finish(Completion::Highlighted(s.diff.clone(), 1, Arc::default()));
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

    #[tokio::test]
    async fn a_reload_highlights_the_file_that_remained_selected() {
        use iced::futures::StreamExt;
        let source = DiffSource::WorkTree("a".into());
        let mut s = state(source.clone());
        let _ = s.update(Message::SelectFile(1));
        let old = s.diff.clone();
        let generation = s.show(source);
        let replacement = Arc::new(LoadedDiff::new(
            parse_diff(DIFF.as_bytes()).expect("diff"),
            false,
        ));
        let task = s.finish(Completion::Loaded(generation, Ok(replacement)));
        let _ = s.finish(Completion::Highlighted(old, 1, Arc::default()));
        assert!(!s.spans.contains_key(&1));
        let mut stream = iced_test::runtime::task::into_stream(task).expect("highlight");
        let Some(iced_test::runtime::Action::Output(done)) = stream.next().await else {
            panic!("highlight completion")
        };
        let _ = s.finish(done);
        assert!(s.spans.contains_key(&1));
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
    fn latest_diff_survives_an_older_failure() {
        let source = DiffSource::WorkTree("a".into());
        let mut s = State::new(Role::Local);
        let old = s.show(source.clone());
        s.show(source.clone());
        let _ = s.loaded(s.generation, Ok(Arc::default()));
        let _ = s.loaded(old, Err("older".into()));
        assert!(s.error.is_none());
    }

    #[test]
    fn closing_and_reopening_rejects_old_content_and_highlights() {
        let source = DiffSource::WorkTree("a".into());
        let mut s = state(source.clone());
        let old = s.generation;
        let old_diff = s.diff.clone();
        s.close();
        let latest = s.show(source);
        let _ = s.finish(Completion::Loaded(old, Ok(old_diff.clone())));
        assert!(s.loading);
        let _ = s.finish(Completion::Loaded(latest, Ok(Arc::default())));
        let _ = s.finish(Completion::Highlighted(old_diff, 1, Arc::default()));
        assert!(!s.spans.contains_key(&1));
        assert!(!s.loading);
    }

    #[test]
    fn stale_results_are_ignored() {
        let mut s = State::new(Role::Local);
        let old = s.show(DiffSource::WorkTree("a".into()));
        s.show(DiffSource::WorkTree("b".into()));
        let _ = s.loaded(old, Ok(Arc::default()));
        assert!(s.loading);
    }

    #[test]
    fn commit_titles_show_only_the_file_name_and_folder() {
        let oid: gf_core::model::Oid = "a".repeat(40).parse().expect("oid");
        let path = std::path::Path::new("crates/gf-app/src/mod.rs");
        let t = Title::of(&DiffSource::Commit(oid.clone()), Some(path));
        assert!(t.lead.is_empty());
        assert_eq!(t.name, "mod.rs");
        assert_eq!(t.rest, "  crates/gf-app/src/");
        assert_eq!(t.full(), "mod.rs  crates/gf-app/src/");

        let t = Title::of(&DiffSource::Commit(oid), None);
        assert!(t.full().is_empty());
    }

    #[test]
    fn local_titles_show_only_the_repository_relative_path() {
        let path = std::path::Path::new("crates/gf-app/src/mod.rs");
        for source in [
            DiffSource::WorkTree(path.into()),
            DiffSource::Staged(path.into()),
            DiffSource::Untracked(path.into()),
        ] {
            assert_eq!(
                Title::of(&source, Some(path)).full(),
                "crates/gf-app/src/mod.rs"
            );
        }
        let top = std::path::Path::new("a.rs");
        assert_eq!(
            Title::of(&DiffSource::WorkTree(top.into()), Some(top)).full(),
            "a.rs"
        );
    }

    /// A diff of `big.txt` adding `lines` lines of `width` bytes, then a
    /// small `g.txt`.
    fn large_diff(lines: usize, width: usize) -> Vec<FileDiff> {
        let mut text = format!(
            "diff --git a/big.txt b/big.txt\nnew file mode 100644\nindex 0000000..1111111\n--- /dev/null\n+++ b/big.txt\n@@ -0,0 +1,{lines} @@\n"
        );
        for _ in 0..lines {
            text.push('+');
            text.push_str(&"x".repeat(width));
            text.push('\n');
        }
        text.push_str(&DIFF[DIFF.find("diff --git a/g.txt").expect("g.txt")..]);
        parse_diff(text.as_bytes()).expect("parse")
    }

    fn loaded(s: &mut State, files: Vec<FileDiff>) {
        let _ = s.loaded(s.generation, Ok(Arc::new(LoadedDiff::new(files, false))));
    }

    #[test]
    fn diff_sizes_count_lines_and_bytes() {
        let size = DiffSize::of(&large_diff(3, 4)[0]);
        assert_eq!(
            size,
            DiffSize {
                lines: 3,
                bytes: 12
            }
        );
        assert!(!size.is_large());
        assert!(DiffSize::of(&large_diff(LARGE_DIFF_LINES + 1, 1)[0]).is_large());
        // One very long line is large too.
        assert!(DiffSize::of(&large_diff(1, LARGE_DIFF_BYTES + 1)[0]).is_large());
        assert_eq!(
            DiffSize {
                lines: 300_000,
                bytes: 10 * 1024 * 1024
            }
            .describe(),
            "300,000 lines · 10.0 MiB"
        );
        assert_eq!(
            DiffSize { lines: 1, bytes: 0 }.describe(),
            "1 line · 0.0 MiB"
        );
    }

    #[test]
    fn a_large_file_waits_until_asked_for_and_stays_shown_on_reload() {
        let source = DiffSource::Commit("a".repeat(40).parse().expect("oid"));
        let mut s = State::new(Role::Commit);
        s.show(source.clone());
        loaded(&mut s, large_diff(LARGE_DIFF_LINES + 1, 1));
        let size = s.held_back().expect("held back");
        assert_eq!(size.lines, LARGE_DIFF_LINES + 1);

        // Nothing hidden can be selected.
        let _ = s.update(Message::SelectAll);
        assert!(!s.has_selection());

        // A small file is drawn at once.
        let _ = s.update(Message::SelectFile(1));
        assert!(s.held_back().is_none());
        let _ = s.update(Message::SelectFile(0));
        assert!(s.held_back().is_some());

        let _ = s.update(Message::ShowLarge);
        assert!(s.held_back().is_none());
        let _ = s.update(Message::SelectAll);
        assert!(s.has_selection());

        // Reloading the same source keeps it shown.
        s.show(source);
        loaded(&mut s, large_diff(LARGE_DIFF_LINES + 1, 1));
        assert!(s.held_back().is_none());

        // Another source asks again.
        s.show(DiffSource::Commit("b".repeat(40).parse().expect("oid")));
        loaded(&mut s, large_diff(LARGE_DIFF_LINES + 1, 1));
        assert!(s.held_back().is_some());
    }

    #[test]
    fn enter_shows_a_held_back_diff_and_is_left_to_the_app_otherwise() {
        let t = crate::theme::preset("gf-dark");
        let mut s = State::new(Role::Local);
        s.show(DiffSource::Untracked("big.txt".into()));
        loaded(&mut s, large_diff(LARGE_DIFF_LINES + 1, 1));
        let enter = Nav::plain(Key::Enter);
        let moved = s.diff_nav(enter, &t).expect("Enter is used");
        assert!(matches!(moved.message, Some(Message::ShowLarge)));
        assert!(s.diff_nav(Nav::plain(Key::Char('n')), &t).is_some());

        let _ = s.update(Message::ShowLarge);
        assert!(s.diff_nav(enter, &t).is_none());
    }
}
