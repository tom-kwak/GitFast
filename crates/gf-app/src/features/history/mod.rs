//! Commit history with a terminal-style graph (`● │ ├─╯`), ref badges and a
//! detail box with actions for the selected commit. Selecting a commit shows
//! its changed files and diff in the commit changes view.
//!
//! The list is virtualized (only visible rows are built) and the graph
//! column has a maximum width, so repositories with many merged branches
//! still show the commit text.
//!
//! History is loaded a page at a time ([`HISTORY_PAGE`] commits): scrolling
//! to the end, or moving the selection onto the last row, asks the app for
//! the next page ([`Output::LoadMore`]).

pub mod file_search;
mod find_file;
pub mod search;

use std::sync::Arc;

use gf_core::model::{Commit, Oid};
use gf_git::graph::GraphRow;
use iced::widget::text::Wrapping;
use iced::widget::{Row, column, container, row};
use iced::{Color, Element, Length, Padding, Task};

use crate::badges::{self, Badge};

use crate::feature::{Cx, Feature, Update};
use crate::features::context_menu::{Entry, Menu};
use crate::nav::{self, Key, Moved, Nav};
use crate::repo_tab::{HISTORY_PAGE, History, Snapshot};
use crate::theme::{Tokens, style};
use crate::widgets::graph_canvas;
use crate::widgets::virtual_list::{self, ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim, format, mono};

/// Scrollable id of the commit list.
pub const LIST_ID: &str = "gf-history-list";
/// Ref badges shown per commit before collapsing into `+N`.
const MAX_BADGES: usize = 2;
/// Badges folded into `+N` that the row's hover lists before `… and N more`.
const HINT_MAX_REFS: usize = 8;
/// Start loading the next page this close (px) to the end of the list.
const NEAR_END_PX: f32 = 200.0;

#[derive(Debug, Default)]
pub struct State {
    /// Revision being shown; `None` = the snapshot's history.
    revision: Option<String>,
    pub file_revision: Option<(std::path::PathBuf, Option<Oid>)>,
    /// Previous History revision when a file was picked from Find File.
    file_history_origin: Option<Option<String>>,
    /// History for `revision`, loaded by the app.
    data: Option<Arc<History>>,
    /// The commit checked out, as of the last update (menus).
    head: Option<Oid>,
    loading: bool,
    /// Single selection, and the anchor for shift-click ranges.
    selected: Option<Oid>,
    /// Multi-selection (shift / ⌘ click); empty when one commit is selected.
    marked: Vec<Oid>,
    /// Row the keyboard is on: the moving end of a shift-range, otherwise
    /// the selected commit.
    cursor: Option<Oid>,
    scroll: ScrollPos,
    /// Author/date/message box of the selected commit; closed by default so
    /// the list keeps its space. The action buttons are always shown.
    show_details: bool,
    /// Commits to load for the snapshot's history (0 = one page).
    head_limit: usize,
    /// Commits to load for `revision` (0 = one page).
    revision_limit: usize,
    /// The next page was requested and has not arrived yet.
    loading_more: bool,
    /// A commit to select and scroll to once a longer history arrives.
    reveal_pending: Option<Oid>,
    search: Option<search::State>,
    file_search: file_search::State,
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(Oid),
    Scrolled(ScrollPos),
    ToggleAllBranches,
    Collapse,
    BranchHere,
    TagHere,
    Checkout,
    CopyId,
    BackToHead,
    RestoreFileHistory,
    ToggleDetails,
    Act(CommitAction),
    /// Shift-click: everything between the anchor and this commit.
    SelectRange(Oid),
    /// ⌘-click: add or remove one commit.
    ToggleMark(Oid),
    Batch(BatchAction),
    /// Right-click on a row: select it (unless it is part of the
    /// multi-selection) and open the actions menu.
    RightClick(Oid),
    /// The `actions ▾` button.
    OpenActions,
    /// Load the next page of history.
    LoadMore,
    /// Open the file search from the history toolbar.
    FindFile,
    Search(search::Message),
}

/// Operations on several selected commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchAction {
    CherryPick,
    Revert,
    /// Meld the selected commits into one (squash dialog).
    Squash,
    /// Open the rebase editor with the selection preset to squash.
    SquashInEditor,
    BranchAtNewest,
    CopyIds,
    Clear,
}

/// Operations on the selected commit that change the repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitAction {
    CherryPick,
    Revert,
    /// Interactive rebase of everything after this commit.
    RebaseFrom,
    /// Start bisecting with HEAD bad and this commit good.
    BisectFrom,
    /// Move the current branch to this commit (`git reset`).
    Reset,
    /// Meld this commit into its parent (squash dialog).
    SquashIntoPrevious,
}

#[derive(Debug, Clone)]
pub enum Output {
    Collapse,
    ShowCommit(Oid),
    ToggleAllBranches,
    NewBranchAt(Oid),
    NewTagAt(Oid),
    CheckoutDetached(Oid),
    CopyId(Oid),
    BackToHead,
    RestoreFileHistory,
    Act(CommitAction, Oid),
    /// Several commits selected: show their combined changes. Newest first.
    ShowRange(Vec<Commit>),
    /// A batch action on the selected commits (newest first).
    Batch(BatchAction, Vec<Commit>),
    /// Open the actions menu; `show` is a newly selected commit to display.
    Menu {
        show: Option<Oid>,
        menu: Menu<Message>,
    },
    /// Reload the history being shown with the (already raised) limit:
    /// the snapshot's when no revision is shown, else the revision's.
    LoadMore,
}

impl State {
    /// Starts showing `revision`; the app loads it and calls [`Self::loaded`].
    pub fn set_revision(&mut self, revision: Option<String>) {
        self.close_file_search();
        self.file_history_origin = None;
        self.loading = revision.is_some();
        self.revision = revision;
        self.file_revision = None;
        self.data = None;
        self.scroll = ScrollPos::default();
        self.revision_limit = 0;
        self.loading_more = false;
    }

    /// Keep the native path for Git, including names that cannot be displayed losslessly.
    pub fn set_file_revision(&mut self, path: std::path::PathBuf, start: Option<Oid>) -> String {
        let label = match &start {
            Some(oid) => format!("file {oid}: {}", path.display()),
            None => format!("file: {}", path.display()),
        };
        self.set_revision(Some(label.clone()));
        self.file_revision = Some((path, start));
        label
    }

    pub fn set_file_revision_from_search(
        &mut self,
        path: std::path::PathBuf,
        start: Oid,
        origin: Option<String>,
    ) -> String {
        let label = self.set_file_revision(path, Some(start));
        self.file_history_origin = Some(origin);
        label
    }

    pub fn restore_file_history(&mut self) -> Option<Option<String>> {
        let origin = self.file_history_origin.take()?;
        self.set_revision(origin.clone());
        Some(origin)
    }

    /// How many commits to load for the snapshot's history.
    pub fn head_limit(&self) -> usize {
        page_or(self.head_limit)
    }

    /// How many commits to load for the revision being shown.
    pub fn revision_limit(&self) -> usize {
        page_or(self.revision_limit)
    }

    /// A snapshot (with its history) arrived, or failed to.
    pub fn snapshot_loaded(&mut self) {
        if self.revision.is_none() {
            self.loading_more = false;
        }
    }

    /// Raises the limit of the history being shown by one page; `false`
    /// when there is nothing more or a page is already on its way.
    fn request_more(&mut self, more: bool) -> bool {
        if !more || self.loading_more || self.loading {
            return false;
        }
        let limit = if self.revision.is_some() {
            &mut self.revision_limit
        } else {
            &mut self.head_limit
        };
        *limit = page_or(*limit) + HISTORY_PAGE;
        self.loading_more = true;
        true
    }

    /// Highlights `oid` in the list (e.g. picked from blame).
    pub fn select(&mut self, oid: Oid) {
        self.cursor = Some(oid.clone());
        self.selected = Some(oid);
        self.marked.clear();
    }

    /// The selected commit (one), if any.
    #[cfg(test)]
    pub fn selected(&self) -> Option<&Oid> {
        self.selected.as_ref()
    }

    /// Selects `oid` and scrolls the list to it (centered when it was out
    /// of view). `None` when the history shown does not have it.
    pub fn reveal<M: Send + 'static>(
        &mut self,
        oid: &Oid,
        snap: &Snapshot,
        t: &Tokens,
    ) -> Option<Task<M>> {
        let i = self
            .history(snap)?
            .commits
            .iter()
            .position(|c| &c.id == oid)?;
        self.select(oid.clone());
        Some(match reveal_center(self.scroll, i, row_height(t)) {
            Some(y) => nav::scroll_y(LIST_ID, y),
            None => Task::none(),
        })
    }

    /// Reveals `oid` once the snapshot's history has at least `rows`
    /// commits: raises the limit to cover them (the app reloads it).
    pub fn reveal_after_load(&mut self, oid: Oid, rows: usize) {
        let pages = rows.div_ceil(HISTORY_PAGE).max(1) + 1;
        self.head_limit = self.head_limit().max(pages * HISTORY_PAGE);
        self.reveal_pending = Some(oid);
    }

    /// The commit waiting for a longer history, once that history arrived:
    /// taken (whether or not it is in it now).
    pub fn take_pending_reveal(&mut self) -> Option<Oid> {
        self.reveal_pending.take()
    }

    /// Arrow keys move the selection (with Shift: extend a range), `.`
    /// opens the actions menu. `None` for keys the list does not use.
    pub fn nav(&self, nav: Nav, snap: Option<&Snapshot>, t: &Tokens) -> Option<Moved<Message>> {
        let commits = &snap.and_then(|s| self.history(s))?.commits;
        self.nav_in(nav, commits, row_height(t))
    }

    fn nav_in(&self, nav: Nav, commits: &[Commit], h: f32) -> Option<Moved<Message>> {
        if nav.key == Key::Char('.') {
            return Some(Moved::message(Message::OpenActions));
        }
        let current = self
            .cursor
            .as_ref()
            .or(self.selected.as_ref())
            .and_then(|id| commits.iter().position(|c| &c.id == id));
        let i = nav::step(current, commits.len(), nav.key, nav::page(self.scroll, h))?;
        if current == Some(i) {
            return Some(Moved::stay());
        }
        let id = commits[i].id.clone();
        let message = if nav.shift {
            Message::SelectRange(id)
        } else {
            Message::Select(id)
        };
        Some(Moved::message(message).with_task(nav::reveal(LIST_ID, self.scroll, i, h)))
    }

    /// The commit `oid` in the list being shown.
    pub fn find<'a>(&'a self, oid: &Oid, snap: Option<&'a Snapshot>) -> Option<&'a Commit> {
        self.history(snap?)?.commits.iter().find(|c| &c.id == oid)
    }

    /// Selected commits in list order (newest first), from `commits`.
    fn marked_commits(&self, commits: &[Commit]) -> Vec<Commit> {
        commits
            .iter()
            .filter(|c| self.marked.contains(&c.id))
            .cloned()
            .collect()
    }

    /// Restore the list position after its widget has been recreated.
    pub fn restore_scroll(&self) -> Task<Message> {
        iced::widget::operation::scroll_to(
            LIST_ID,
            iced::widget::scrollable::AbsoluteOffset {
                x: 0.0,
                y: self.scroll.offset,
            },
        )
    }

    /// Range/toggle selection needs the list being shown.
    pub fn update_with(
        &mut self,
        message: Message,
        snap: Option<&Snapshot>,
    ) -> Update<Message, Output> {
        self.head = snap.and_then(|s| s.status.head.oid.clone());
        // Shared, not copied: this runs for every scroll event.
        let shown = snap.and_then(|s| match (&self.revision, &self.data) {
            (None, _) => Some(s.history.clone()),
            (Some(_), d) => d.clone(),
        });
        let (commits, more): (&[Commit], bool) = match &shown {
            Some(h) => (&h.commits, h.more),
            None => (&[], false),
        };
        // Reaching the end (by scrolling or by moving onto the last row)
        // loads the next page.
        let at_end = match &message {
            Message::Scrolled(pos) => pos.near_end(NEAR_END_PX),
            Message::Select(id) | Message::SelectRange(id) => {
                commits.last().is_some_and(|c| &c.id == id)
            }
            _ => false,
        };
        if matches!(message, Message::LoadMore) {
            return if self.request_more(more) {
                Update::output(Output::LoadMore)
            } else {
                Update::none()
            };
        }
        let mut up = self.update_list(message, commits);
        if at_end && more && !self.loading_more && !self.loading {
            up.task = Task::batch([up.task, Task::done(Message::LoadMore)]);
        }
        up
    }

    /// `update_with` for a given list of the commits shown (newest first).
    fn update_list(&mut self, message: Message, commits: &[Commit]) -> Update<Message, Output> {
        let index = |id: &Oid| commits.iter().position(|c| &c.id == id);
        let after = |state: &mut Self| -> Update<Message, Output> {
            match state.marked.len() {
                0 => Update::none(),
                1 => {
                    let only = state.marked.remove(0);
                    state.selected = Some(only.clone());
                    Update::output(Output::ShowCommit(only))
                }
                _ => Update::output(Output::ShowRange(state.marked_commits(commits))),
            }
        };
        match message {
            Message::SelectRange(oid) => {
                self.cursor = Some(oid.clone());
                let (Some(anchor), Some(to)) =
                    (self.selected.as_ref().and_then(index), index(&oid))
                else {
                    return self.update(Message::Select(oid));
                };
                let (lo, hi) = (anchor.min(to), anchor.max(to));
                self.marked = commits[lo..=hi].iter().map(|c| c.id.clone()).collect();
                after(self)
            }
            Message::ToggleMark(oid) => {
                self.cursor = Some(oid.clone());
                if self.marked.is_empty()
                    && let Some(sel) = self.selected.clone()
                {
                    self.marked.push(sel);
                }
                if let Some(pos) = self.marked.iter().position(|m| m == &oid) {
                    self.marked.remove(pos);
                } else {
                    self.marked.push(oid);
                }
                // Keep list order (newest first).
                self.marked.sort_by_key(|m| index(m).unwrap_or(usize::MAX));
                after(self)
            }
            Message::Batch(BatchAction::Clear) => {
                self.marked.clear();
                Update::none()
            }
            Message::Batch(action) => {
                let selected = self.marked_commits(commits);
                if selected.is_empty() {
                    return Update::none();
                }
                Update::output(Output::Batch(action, selected))
            }
            Message::RightClick(oid) if self.marked.len() > 1 && self.marked.contains(&oid) => {
                Update::output(Output::Menu {
                    show: None,
                    menu: batch_menu(&self.marked_commits(commits)),
                })
            }
            Message::RightClick(oid) => {
                let Some(c) = commits.iter().find(|c| c.id == oid) else {
                    return Update::none();
                };
                let changed = self.selected.as_ref() != Some(&oid) || !self.marked.is_empty();
                self.cursor = Some(oid.clone());
                self.selected = Some(oid.clone());
                self.marked.clear();
                Update::output(Output::Menu {
                    show: changed.then_some(oid),
                    menu: commit_menu(c, self.head.as_ref() == Some(&c.id)),
                })
            }
            Message::OpenActions if self.marked.len() > 1 => Update::output(Output::Menu {
                show: None,
                menu: batch_menu(&self.marked_commits(commits)),
            }),
            Message::OpenActions => {
                let c = self
                    .selected
                    .as_ref()
                    .and_then(|s| commits.iter().find(|c| &c.id == s));
                match c {
                    Some(c) => Update::output(Output::Menu {
                        show: None,
                        menu: commit_menu(c, self.head.as_ref() == Some(&c.id)),
                    }),
                    None => Update::none(),
                }
            }
            other => self.update(other),
        }
    }

    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    /// History loaded for `revision`, when that is the one shown.
    pub fn revision_history(&self, revision: &str) -> Option<&Arc<History>> {
        self.data
            .as_ref()
            .filter(|_| self.revision.as_deref() == Some(revision))
    }

    /// The commit whose tree should supply file-search candidates.
    pub fn loaded(&mut self, revision: &str, data: Arc<History>) {
        // Ignore results for a revision the user already navigated away from.
        if self.revision.as_deref() == Some(revision) {
            self.data = Some(data);
            self.loading = false;
            self.loading_more = false;
        }
    }

    pub fn load_failed(&mut self) {
        // A failed next page keeps what is already shown.
        if self.loading_more {
            self.loading_more = false;
            return;
        }
        self.loading = false;
        self.revision = None;
        self.file_revision = None;
        self.file_history_origin = None;
    }

    fn history<'a>(&'a self, snap: &'a Snapshot) -> Option<&'a History> {
        match (&self.revision, &self.data) {
            (None, _) => Some(&snap.history),
            (Some(_), Some(d)) => Some(d),
            (Some(_), None) => None,
        }
    }
}

/// Offset that shows row `index` in the middle of the list, or `None` when
/// it is already fully in view.
fn reveal_center(pos: ScrollPos, index: usize, h: f32) -> Option<f32> {
    nav::reveal_offset(pos, index, h)?;
    #[allow(clippy::cast_precision_loss)]
    let top = index as f32 * h;
    Some((top - (pos.height - h) / 2.0).max(0.0))
}

/// `limit`, or one page when unset.
fn page_or(limit: usize) -> usize {
    if limit == 0 { HISTORY_PAGE } else { limit }
}

/// Row height for the list: one line of text plus padding.
fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.7).round()
}

/// The color of a badge.
fn badge_color(t: &Tokens, kind: badges::Kind) -> Color {
    match kind {
        badges::Kind::Head => t.head(),
        badges::Kind::Branch => t.branch(),
        badges::Kind::Remote => t.remote(),
        badges::Kind::Tag => t.tag(),
    }
}

/// Columns reserved for the author and date.
const AUTHOR_COLS: usize = 16;
const DATE_COLS: usize = 16;
/// Badge labels longer than this many columns end in `…`.
const BADGE_COLS: usize = 28;
/// The summary column never gets narrower than this; below it the list
/// scrolls sideways instead.
const MIN_SUMMARY_COLS: usize = 20;
/// Columns the summary text keeps after the badges, however many there are.
const MIN_TEXT_COLS: usize = 10;

/// Badges and summary as one text line, cut to fit `budget` columns.
struct SummaryLine {
    parts: Vec<(String, Option<badges::Kind>)>,
    /// Display width in columns.
    cols: usize,
    /// Whether the summary ends in `…`.
    cut: bool,
}

/// The badges of `c` (each label cut at [`BADGE_COLS`]) and its summary,
/// cut with `…` where the line would pass `budget` columns.
/// `usize::MAX` measures the line uncut.
/// Columns of `c`'s summary line with all the room it wants: what the list
/// needs to show the whole of it.
pub(crate) fn summary_cols(c: &Commit, badges: Option<&Vec<Badge>>) -> usize {
    summary_line(c, badges, usize::MAX).cols
}

fn summary_line(c: &Commit, badges: Option<&Vec<Badge>>, budget: usize) -> SummaryLine {
    let mut parts: Vec<(String, Option<badges::Kind>)> = Vec::new();
    if let Some(refs) = badges {
        for b in refs.iter().take(MAX_BADGES) {
            parts.push((
                format!("({}) ", format::truncate_columns(&b.label, BADGE_COLS)),
                Some(b.kind),
            ));
        }
        if refs.len() > MAX_BADGES {
            parts.push((format!("+{} ", refs.len() - MAX_BADGES), None));
        }
    }
    let badge_cols: usize = parts.iter().map(|(s, _)| columns(s)).sum();
    let room = budget.saturating_sub(badge_cols).max(MIN_TEXT_COLS);
    let text = format::truncate_columns(&c.summary, room);
    let cut = text != c.summary;
    let cols = badge_cols + columns(&text);
    parts.push((text, None));
    SummaryLine { parts, cols, cut }
}

#[allow(clippy::too_many_arguments)]
fn commit_row<'a>(
    t: &'a Tokens,
    c: &'a Commit,
    g: Option<&'a GraphRow>,
    graph_px: f32,
    summary_cols: usize,
    badges: Option<&Vec<Badge>>,
    unpushed: bool,
    selected: bool,
) -> Element<'a, Message> {
    let mut line = Row::new()
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .height(Length::Fill);
    if let Some(g) = g {
        let colors = [0, 1, 2, 3, 4, 5].map(|i| t.lane(i));
        line = line.push(graph_canvas::row_graph(g, colors, t.bg, graph_px));
    }
    line = line.push(colored(t, c.id.short(), t.warning()).wrapping(Wrapping::None));
    let marker = container(
        colored(
            t,
            if unpushed { widgets::PUSH_ARROW } else { " " },
            t.success(),
        )
        .wrapping(Wrapping::None),
    )
    .width(Length::Fixed(t.cols(1)));
    line = line.push(if unpushed {
        widgets::tip(
            t,
            marker,
            "Reachable from the checked-out branch, but not from any local remote-tracking branch."
                .into(),
        )
    } else {
        marker.into()
    });

    let line_text = summary_line(c, badges, summary_cols);
    let hint = row_hint(c, badges, line_text.cut);
    let mut summary = Row::new();
    for (text, kind) in line_text.parts {
        summary = summary.push(match kind {
            Some(kind) => colored(t, text, badge_color(t, kind)).wrapping(Wrapping::None),
            None => mono(t, text).wrapping(Wrapping::None),
        });
    }
    let summary = container(summary)
        .width(Length::Fixed(t.cols(summary_cols)))
        .clip(true);
    line = line
        .push(match hint {
            Some(hint) => widgets::tip(t, summary, hint),
            None => summary.into(),
        })
        .push(
            container(
                dim(t, format::truncate(&c.author.name, AUTHOR_COLS)).wrapping(Wrapping::None),
            )
            .width(Length::Fixed(t.cols(AUTHOR_COLS))),
        )
        .push(dim(t, format::datetime(c.author.time)).wrapping(Wrapping::None));
    iced::widget::mouse_area(widgets::list_row_flush(
        t,
        line,
        selected,
        Message::Select(c.id.clone()),
    ))
    .on_right_press(Message::RightClick(c.id.clone()))
    .into()
}

/// What hovering a row's summary shows, top to bottom:
/// - the full label of a shown badge cut at [`BADGE_COLS`],
/// - the badges folded into `+N`, one a line (at most [`HINT_MAX_REFS`], then
///   `… and N more`),
/// - after a blank line, the summary when it did not fit the column
///   (`summary_cut`).
///
/// `None` when nothing was cut or folded. Every ref and the message body are
/// in the detail box (`▸ details`).
fn row_hint(c: &Commit, badges: Option<&Vec<Badge>>, summary_cut: bool) -> Option<String> {
    let refs = badges.map_or(&[][..], Vec::as_slice);
    let (shown, hidden) = refs.split_at(refs.len().min(MAX_BADGES));
    let mut lines: Vec<String> = shown
        .iter()
        .map(|b| b.label.as_str())
        .filter(|l| columns(l) > BADGE_COLS)
        .map(str::to_owned)
        .collect();
    lines.extend(hidden.iter().take(HINT_MAX_REFS).map(|b| b.label.clone()));
    if hidden.len() > HINT_MAX_REFS {
        lines.push(format!("… and {} more", hidden.len() - HINT_MAX_REFS));
    }
    if summary_cut {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(c.summary.clone());
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Columns for the summary column: as many as `room_px` holds, no more
/// than the `widest` line needs (short histories keep author and date
/// close) and never fewer than [`MIN_SUMMARY_COLS`].
fn summary_cols_for(room_px: f32, char_px: f32, widest: usize) -> usize {
    // Truncation is intended: whole columns that fit.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fits = if char_px > 0.0 {
        (room_px.max(0.0) / char_px) as usize
    } else {
        0
    };
    fits.min(widest).max(MIN_SUMMARY_COLS)
}

/// `[ actions ▾ ]`: opens the same menu as right-clicking the selection.
fn actions_button(t: &Tokens) -> Element<'_, Message> {
    widgets::tip(
        t,
        widgets::text_button(t, "actions ▾", None, Some(Message::OpenActions)),
        "all actions for the selection · or right-click a commit".into(),
    )
}

/// Actions for one commit; `is_head`: it is the commit checked out.
fn commit_menu(c: &Commit, is_head: bool) -> Menu<Message> {
    let act = |a| Message::Act(a);
    let mut menu = Menu {
        title: format!("{} {}", c.id.short(), format::truncate(&c.summary, 40)),
        entries: vec![
            Entry::item(
                'b',
                "branch here…",
                "create a branch at this commit",
                Message::BranchHere,
            ),
            Entry::item(
                't',
                "tag here…",
                "create a tag at this commit",
                Message::TagHere,
            ),
            Entry::item(
                'o',
                "checkout (detached)",
                "switch your files to this commit without a branch",
                Message::Checkout,
            ),
            Entry::Separator,
            Entry::item(
                'p',
                "cherry-pick",
                "apply this commit on top of your branch",
                act(CommitAction::CherryPick),
            ),
            Entry::item(
                'r',
                "revert",
                "add a new commit that undoes this one",
                act(CommitAction::Revert),
            ),
            Entry::caution(
                'q',
                "squash into previous…",
                "meld this commit into the one before it",
                act(CommitAction::SquashIntoPrevious),
            ),
            Entry::caution(
                'i',
                "rebase -i from here…",
                "reorder / squash / reword / drop the commits after this one",
                act(CommitAction::RebaseFrom),
            ),
            Entry::item(
                's',
                "bisect from here",
                "find the commit that broke something: HEAD is bad, this one is good",
                act(CommitAction::BisectFrom),
            ),
            Entry::caution(
                'e',
                "reset branch here…",
                "move the current branch to this commit (soft, mixed or hard)",
                act(CommitAction::Reset),
            ),
            Entry::Separator,
            Entry::item('y', "copy id", "copy the full commit id", Message::CopyId),
        ],
    };
    // Bisecting needs a good commit before HEAD (the bad one).
    if is_head {
        menu.entries
            .retain(|e| !matches!(e, Entry::Item { key: 's', .. }));
    }
    menu
}

/// Actions for several commits (newest first).
fn batch_menu(commits: &[Commit]) -> Menu<Message> {
    let n = commits.len();
    let b = Message::Batch;
    Menu {
        title: format!("{n} commits selected"),
        entries: vec![
            Entry::item(
                'p',
                format!("cherry-pick {n}"),
                "apply them, oldest first, on top of your branch",
                b(BatchAction::CherryPick),
            ),
            Entry::item(
                'r',
                format!("revert {n}"),
                "add commits that undo them, newest first",
                b(BatchAction::Revert),
            ),
            Entry::caution(
                'q',
                format!("squash {n} into one…"),
                "meld them into a single commit with a new message",
                b(BatchAction::Squash),
            ),
            Entry::caution(
                'i',
                "squash in rebase editor…",
                "the full plan: reorder, reword or drop the others too",
                b(BatchAction::SquashInEditor),
            ),
            Entry::Separator,
            Entry::item(
                'b',
                "branch at newest…",
                "create a branch at the newest selected commit",
                b(BatchAction::BranchAtNewest),
            ),
            Entry::item(
                'y',
                "copy ids",
                "copy all selected commit ids",
                b(BatchAction::CopyIds),
            ),
            Entry::Separator,
            Entry::item(
                'x',
                "clear selection",
                "back to a single commit",
                b(BatchAction::Clear),
            ),
        ],
    }
}

/// Bar for several selected commits: what is selected and batch actions.
fn batch_bar<'a>(t: &'a Tokens, commits: &[Commit]) -> Element<'a, Message> {
    let n = commits.len();
    let range = match (commits.last(), commits.first()) {
        (Some(oldest), Some(newest)) => format!("{} … {}", oldest.id.short(), newest.id.short()),
        _ => String::new(),
    };
    let head = row![
        colored(t, format!("{n} commits selected"), t.accent()),
        dim(t, range),
        dim(t, "· shift-click range · ⌘-click add/remove").width(Length::Fill),
        actions_button(t),
        widgets::icon_button(
            t,
            "✕",
            t.dim,
            "clear selection",
            Message::Batch(BatchAction::Clear)
        ),
    ]
    .spacing(8);
    container(head)
        .padding(Padding::from([2, 6]))
        .width(Length::Fill)
        .clip(true)
        .style(style::bar(t))
        .into()
}

/// Bar under the list for the selected commit: id, actions and a details
/// toggle; with details open, also author, date, every ref on the commit
/// (the row folds all but [`MAX_BADGES`]) and the full message.
fn detail<'a>(
    t: &'a Tokens,
    c: &'a Commit,
    refs: Option<&'a Vec<Badge>>,
    open: bool,
) -> Element<'a, Message> {
    let bar = row![
        colored(t, c.id.short(), t.warning()),
        mono(t, c.summary.clone())
            .wrapping(Wrapping::None)
            .width(Length::Fill),
        actions_button(t),
        widgets::tip(
            t,
            widgets::text_button(
                t,
                if open { "▾ details" } else { "▸ details" },
                None,
                Some(Message::ToggleDetails),
            ),
            if open {
                "hide author, date and message"
            } else {
                "show author, date and full message"
            }
            .into(),
        ),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);

    if !open {
        return container(bar)
            .padding(Padding::from([2, 6]))
            .width(Length::Fill)
            .clip(true)
            .style(style::bar(t))
            .into();
    }

    let mut col = column![
        colored(t, format!("commit {}", c.id), t.warning()).wrapping(Wrapping::None),
        mono(t, format!("Author: {} <{}>", c.author.name, c.author.email)),
        mono(t, format!("Date:   {}", format::datetime(c.author.time))),
    ];
    if c.is_merge() {
        let parents: Vec<_> = c.parents.iter().map(Oid::short).collect();
        col = col.push(mono(t, format!("Merge:  {}", parents.join(" "))));
    }
    if c.committer.email != c.author.email {
        col = col.push(dim(
            t,
            format!("Committed by {} <{}>", c.committer.name, c.committer.email),
        ));
    }
    for (i, b) in refs.into_iter().flatten().enumerate() {
        let head = if i == 0 { "Refs:   " } else { "        " };
        col = col.push(colored(
            t,
            format!("{head}{}", b.label),
            badge_color(t, b.kind),
        ));
    }
    col = col.push(mono(t, format!("\n    {}", c.summary)));
    for line in c.body.lines() {
        col = col.push(mono(t, format!("    {line}")));
    }
    container(column![bar, widgets::scroll(col.spacing(1))].spacing(4))
        .padding(Padding::from([4, 6]))
        .width(Length::Fill)
        .height(Length::Fixed(t.font_size * 10.0))
        .style(style::bar(t))
        .into()
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        let selected = self.selected.clone();
        match message {
            Message::Select(oid) => {
                self.cursor = Some(oid.clone());
                self.selected = Some(oid.clone());
                self.marked.clear();
                Update::output(Output::ShowCommit(oid))
            }
            // Need the commit list: handled in `update_with`.
            Message::SelectRange(_)
            | Message::ToggleMark(_)
            | Message::Batch(_)
            | Message::RightClick(_)
            | Message::OpenActions
            | Message::LoadMore => Update::none(),
            Message::Scrolled(pos) => {
                self.scroll = pos;
                Update::none()
            }
            Message::ToggleAllBranches => Update::output(Output::ToggleAllBranches),
            Message::Collapse => Update::output(Output::Collapse),
            Message::BranchHere => {
                selected.map_or_else(Update::none, |o| Update::output(Output::NewBranchAt(o)))
            }
            Message::TagHere => {
                selected.map_or_else(Update::none, |o| Update::output(Output::NewTagAt(o)))
            }
            Message::Checkout => selected.map_or_else(Update::none, |o| {
                Update::output(Output::CheckoutDetached(o))
            }),
            Message::CopyId => {
                selected.map_or_else(Update::none, |o| Update::output(Output::CopyId(o)))
            }
            Message::BackToHead => Update::output(Output::BackToHead),
            Message::RestoreFileHistory => Update::output(Output::RestoreFileHistory),
            Message::Act(action) => {
                selected.map_or_else(Update::none, |o| Update::output(Output::Act(action, o)))
            }
            Message::ToggleDetails => {
                self.show_details = !self.show_details;
                Update::none()
            }
            Message::FindFile => Update::none(),
            Message::Search(message) => {
                if let Some(search) = self.search.as_mut() {
                    search.update(message);
                }
                Update::none()
            }
        }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let all = cx.all_branches;
        let scope = if let (Some((path, _)), Some(origin)) =
            (&self.file_revision, &self.file_history_origin)
        {
            let base = origin.as_deref().map_or("HEAD", revision_label);
            format!("{base} · {}", path.display())
        } else {
            match &self.revision {
                Some(r) => revision_label(r).to_owned(),
                None if all => "all branches".into(),
                None => "HEAD".into(),
            }
        };
        let mut toolbar = row![dim(t, format!("showing: {scope}")).width(Length::Fill)]
            .spacing(4)
            .align_y(iced::Alignment::Center);
        if self.file_history_origin.is_some() {
            toolbar = toolbar.push(widgets::icon_button(
                t,
                "×",
                t.dim,
                "Esc: return to previous history",
                Message::RestoreFileHistory,
            ));
        } else if self.revision.is_some() {
            toolbar = toolbar.push(widgets::text_button(
                t,
                "back to all",
                None,
                Some(Message::BackToHead),
            ));
        } else {
            toolbar = toolbar.push(widgets::text_button(
                t,
                if all {
                    "■ all branches"
                } else {
                    "□ all branches"
                },
                None,
                Some(Message::ToggleAllBranches),
            ));
        }
        if self.search.is_none() && self.file_revision.is_none() {
            toolbar = toolbar.push(widgets::tip(
                t,
                widgets::text_button(t, "Find", Some("⌘F"), Some(Message::FindFile)),
                "⌘F: search the files in this history's latest commit, then show the commits that changed one"
                    .into(),
            ));
        }

        let panel = |title: String, content: Element<'a, Message>| {
            widgets::panel_with_action(
                t,
                title,
                widgets::icon_button(t, "▴", t.accent(), "hide history ⌘⇧H", Message::Collapse),
                content,
            )
        };
        let Some(snap) = cx.snapshot else {
            return panel("HISTORY".into(), widgets::empty_hint(t, "loading…"));
        };
        // The search owns the panel even while a branch history loads: its
        // field has the keyboard.
        if self.search.is_some() {
            return panel(
                "HISTORY".into(),
                column![toolbar, self.file_search_view(t)].into(),
            );
        }
        if self.loading {
            return panel(
                "HISTORY".into(),
                column![toolbar, widgets::empty_hint(t, "loading history…")].into(),
            );
        }
        let Some(history) = self.history(snap) else {
            return panel("HISTORY".into(), toolbar.into());
        };
        if history.commits.is_empty() {
            return panel(
                "HISTORY".into(),
                column![
                    toolbar,
                    widgets::empty_hint(
                        t,
                        "no commits yet — open the working tree (⌘E) to stage and commit"
                    )
                ]
                .into(),
            );
        }

        let decor = &snap.badges;
        // Worked out while the history loaded (walking every commit here
        // took 50 ms on the first frame of an 80 000-commit history).
        let (lanes, widest) = (history.lanes, history.widest);
        let graph_px = graph_canvas::width_for(lanes);
        // graph, hash, marker, author, date + gaps and padding: all but the summary.
        let fixed_px = graph_px + t.cols(8 + AUTHOR_COLS + DATE_COLS) + 5.0 * 6.0 + 12.0;
        let selected = self.selected.as_ref();
        let marked = &self.marked;
        let unpushed = &snap.unpushed_commits;
        let is_selected = move |id: &Oid| {
            if marked.is_empty() {
                selected == Some(id)
            } else {
                marked.contains(id)
            }
        };
        let total = history.commits.len();
        let scroll = self.scroll;
        // The summary takes the room the window leaves it: wider windows
        // show more of a long summary before the `…`.
        let list = iced::widget::responsive(move |size| {
            let room = size.width - virtual_list::SCROLLBAR - fixed_px;
            let summary_cols = summary_cols_for(room, t.char_width(), widest);
            let content_width = fixed_px + t.cols(summary_cols);
            virtual_list(
                LIST_ID,
                total,
                row_height(t),
                content_width,
                scroll,
                move |i| {
                    let c = &history.commits[i];
                    commit_row(
                        t,
                        c,
                        history.graph.get(i),
                        graph_px,
                        summary_cols,
                        decor.get(&c.id),
                        unpushed.contains(&c.id),
                        is_selected(&c.id),
                    )
                },
                Message::Scrolled,
            )
        });

        let mut content = column![toolbar, list].spacing(4);
        if self.marked.len() > 1 {
            content = content.push(batch_bar(t, &self.marked_commits(&history.commits)));
        } else if let Some(c) = self
            .selected
            .as_ref()
            .and_then(|s| history.commits.iter().find(|c| &c.id == s))
        {
            content = content.push(detail(t, c, decor.get(&c.id), self.show_details));
        }
        let count = history.commits.len();
        let more = if history.more { "+" } else { "" };
        panel(format!("HISTORY ({count}{more})"), content.into())
    }
}

/// How a revision reads in `showing:`: a ref by its full short name
/// (`feature/login`, `origin/main`), a commit id by its first 7 digits.
fn revision_label(rev: &str) -> &str {
    let short = ["refs/heads/", "refs/remotes/", "refs/tags/"]
        .iter()
        .find_map(|p| rev.strip_prefix(p))
        .unwrap_or(rev);
    if short.len() == 40 && short.bytes().all(|b| b.is_ascii_hexdigit()) {
        &short[..7]
    } else {
        short
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn revision_labels_keep_the_branch_folder_and_shorten_ids() {
        assert_eq!(super::revision_label("refs/heads/agent/docs"), "agent/docs");
        assert_eq!(
            super::revision_label("refs/remotes/origin/main"),
            "origin/main"
        );
        assert_eq!(super::revision_label(&"a".repeat(40)), "aaaaaaa");
        assert_eq!(super::revision_label("HEAD~2"), "HEAD~2");
    }

    use super::*;

    #[test]
    fn reveal_selects_the_commit_and_scrolls_only_when_out_of_view() {
        let t = crate::theme::preset("gf-dark");
        let h = row_height(&t);
        let list = commits(50);
        let mut s = shown(list.clone(), false);
        let snap = Snapshot::default();
        // A viewport of ten rows at the top.
        s.scroll = ScrollPos {
            offset: 0.0,
            height: h * 10.0,
            content: h * 50.0,
        };
        let target = list[40].id.clone();
        assert!(s.reveal::<Message>(&target, &snap, &t).is_some());
        assert_eq!(s.selected(), Some(&target));
        assert!(s.marked.is_empty());
        let missing: Oid = "f".repeat(40).parse().expect("oid");
        assert!(
            s.reveal::<Message>(&missing, &snap, &t).is_none(),
            "not in the list"
        );
        assert_eq!(s.selected(), Some(&target), "selection kept when not found");
    }

    #[test]
    fn a_revealed_row_lands_in_the_middle_unless_already_visible() {
        let h = 10.0;
        let view = |offset| ScrollPos {
            offset,
            height: 100.0,
            content: 10_000.0,
        };
        assert_eq!(reveal_center(view(0.0), 3, h), None, "visible: stay");
        // Row 40 (top 400): centered means 400 - (100 - 10) / 2 = 355.
        assert_eq!(reveal_center(view(0.0), 40, h), Some(355.0));
        assert_eq!(reveal_center(view(900.0), 40, h), Some(355.0), "above too");
        assert_eq!(
            reveal_center(view(500.0), 2, h),
            Some(0.0),
            "never above the top"
        );
    }

    #[test]
    fn revealing_after_a_load_raises_the_limit_past_the_row() {
        let mut s = State::default();
        let oid: Oid = "a".repeat(40).parse().expect("oid");
        s.reveal_after_load(oid.clone(), 1_800);
        assert!(
            s.head_limit() >= 1_800 + HISTORY_PAGE / 2,
            "{}",
            s.head_limit()
        );
        assert_eq!(s.head_limit() % HISTORY_PAGE, 0, "whole pages");
        // Never lowered by a nearer one.
        let before = s.head_limit();
        s.reveal_after_load(oid.clone(), 5);
        assert_eq!(s.head_limit(), before);
        assert_eq!(s.take_pending_reveal(), Some(oid));
        assert_eq!(s.take_pending_reveal(), None, "taken once");
    }

    #[test]
    fn stale_revision_results_are_ignored() {
        let mut s = State::default();
        s.set_revision(Some("refs/heads/a".into()));
        s.set_revision(Some("refs/heads/b".into()));
        s.loaded("refs/heads/a", Arc::default());
        assert!(s.loading, "result for a must not complete b");
        s.loaded("refs/heads/b", Arc::default());
        assert!(!s.loading);
    }

    #[test]
    fn actions_need_a_selection() {
        let mut s = State::default();
        assert!(s.update(Message::BranchHere).output.is_none());
        let oid: Oid = "a".repeat(40).parse().expect("oid");
        let _ = s.update(Message::Select(oid.clone()));
        assert!(matches!(s.update(Message::CopyId).output, Some(Output::CopyId(o)) if o == oid));
    }

    fn shown(commits: Vec<Commit>, more: bool) -> State {
        let mut s = State::default();
        s.set_revision(Some("refs/heads/x".into()));
        s.loaded(
            "refs/heads/x",
            Arc::new(History {
                more,
                ..History::new(commits)
            }),
        );
        s
    }

    #[test]
    fn scrolling_to_the_end_loads_one_more_page() {
        let snap = Snapshot::default();
        let mut s = shown(commits(5), true);
        assert_eq!(s.revision_limit(), HISTORY_PAGE);
        let near_top = ScrollPos {
            offset: 0.0,
            height: 100.0,
            content: 2000.0,
        };
        let up = s.update_with(Message::Scrolled(near_top), Some(&snap));
        assert!(up.output.is_none());
        assert!(!s.loading_more);

        let at_end = ScrollPos {
            offset: 1850.0,
            ..near_top
        };
        let _ = s.update_with(Message::Scrolled(at_end), Some(&snap));
        // The scroll asks (through a task) for the next page.
        let up = s.update_with(Message::LoadMore, Some(&snap));
        assert!(matches!(up.output, Some(Output::LoadMore)));
        assert_eq!(s.revision_limit(), 2 * HISTORY_PAGE);
        assert_eq!(s.head_limit(), HISTORY_PAGE, "only the shown history grows");

        // While the page is on its way, no second request.
        assert!(
            s.update_with(Message::LoadMore, Some(&snap))
                .output
                .is_none()
        );
        s.loaded(
            "refs/heads/x",
            Arc::new(History {
                more: false,
                ..History::new(commits(8))
            }),
        );
        assert!(!s.loading_more);
        // The whole history is shown: nothing more to load.
        assert!(
            s.update_with(Message::LoadMore, Some(&snap))
                .output
                .is_none()
        );
        assert_eq!(s.revision_limit(), 2 * HISTORY_PAGE);
    }

    #[test]
    fn failed_page_keeps_the_history_shown() {
        let snap = Snapshot::default();
        let mut s = shown(commits(3), true);
        let _ = s.update_with(Message::LoadMore, Some(&snap));
        s.load_failed();
        assert_eq!(s.revision(), Some("refs/heads/x"));
        assert!(!s.loading_more);
    }

    #[test]
    fn changing_revision_resets_scroll() {
        let mut s = State::default();
        let _ = s.update(Message::Scrolled(ScrollPos {
            offset: 500.0,
            height: 300.0,
            ..ScrollPos::default()
        }));
        s.set_revision(Some("refs/heads/x".into()));
        assert_eq!(s.scroll, ScrollPos::default());
    }

    #[test]
    fn bisect_is_offered_only_below_head() {
        let c = &commits(1)[0];
        let keys = |m: Menu<Message>| -> String {
            m.entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item { key, .. } => Some(*key),
                    Entry::Separator => None,
                })
                .collect()
        };
        assert!(keys(commit_menu(c, false)).contains('s'));
        // HEAD would be both the bad and the good commit: git refuses.
        assert!(!keys(commit_menu(c, true)).contains('s'));
    }

    fn commits(n: u8) -> Vec<Commit> {
        let sig = gf_core::model::Signature {
            name: String::new(),
            email: String::new(),
            time: gf_core::model::Timestamp {
                seconds: 0,
                offset_minutes: 0,
            },
        };
        (0..n)
            .map(|i| Commit {
                id: format!("{i:040x}").parse().expect("oid"),
                parents: vec![],
                author: sig.clone(),
                committer: sig.clone(),
                summary: format!("c{i}"),
                body: String::new(),
            })
            .collect()
    }

    #[test]
    fn summaries_are_cut_to_the_room_and_shown_whole_on_hover() {
        let mut c = commits(1).remove(0);
        c.summary = "x".repeat(40);
        let fit = summary_line(&c, None, 40);
        assert!(!fit.cut);
        assert_eq!(row_hint(&c, None, fit.cut), None, "it fits: no hover");

        let narrow = summary_line(&c, None, 30);
        assert!(narrow.cut);
        assert_eq!(narrow.cols, 30);
        assert!(narrow.parts[0].0.ends_with('…'));
        c.body = "why".into();
        // Only the summary line: the body is in the detail box.
        assert_eq!(row_hint(&c, None, true), Some(c.summary.clone()));

        // Korean takes two columns a character.
        c.summary = "가".repeat(20);
        assert!(summary_line(&c, None, 39).cut);
        assert!(!summary_line(&c, None, 40).cut);
        assert_eq!(summary_line(&c, None, usize::MAX).cols, 40);
    }

    #[test]
    fn badges_take_room_first_but_leave_some_for_the_summary() {
        let mut c = commits(1).remove(0);
        c.summary = "s".repeat(30);
        let refs = vec![Badge {
            label: "main".into(),
            kind: badges::Kind::Branch,
        }];
        // `(main) ` is 7 columns: 23 left for the summary.
        let line = summary_line(&c, Some(&refs), 30);
        assert_eq!(line.cols, 30);
        assert!(line.cut);
        // However narrow, the summary keeps MIN_TEXT_COLS.
        let line = summary_line(&c, Some(&refs), 0);
        assert_eq!(line.cols, 7 + MIN_TEXT_COLS);
    }

    #[test]
    fn the_summary_column_follows_the_window_width() {
        // 8px a column.
        assert_eq!(summary_cols_for(800.0, 8.0, 500), 100);
        assert_eq!(summary_cols_for(400.0, 8.0, 500), 50);
        assert_eq!(summary_cols_for(800.0, 8.0, 30), 30, "no wider than needed");
        assert_eq!(summary_cols_for(-50.0, 8.0, 500), MIN_SUMMARY_COLS);
    }

    #[test]
    fn only_cut_or_folded_text_shows_on_hover() {
        let c = commits(1).remove(0);
        let badge = |l: &str| Badge {
            label: l.into(),
            kind: badges::Kind::Branch,
        };
        let long = "b".repeat(BADGE_COLS + 1);
        // Everything shown whole: no hover.
        let refs = vec![badge("a"), badge("b")];
        assert_eq!(row_hint(&c, Some(&refs), false), None);
        let refs = vec![badge("a"), badge(&long)];
        assert_eq!(row_hint(&c, Some(&refs), false), Some(long.clone()));
        let line = summary_line(&c, Some(&refs), usize::MAX);
        assert!(line.parts[1].0.ends_with("…) "));
    }

    #[test]
    fn the_hover_lists_the_refs_folded_into_plus_n() {
        let mut c = commits(1).remove(0);
        let badge = |l: &str| Badge {
            label: l.into(),
            kind: badges::Kind::Tag,
        };
        // `+1`: the folded ref's name, nothing else.
        let refs = vec![badge("a"), badge("b"), badge("◆ v1")];
        assert_eq!(row_hint(&c, Some(&refs), false), Some("◆ v1".into()));

        // Folded refs, a blank line, then the cut summary.
        c.summary = "whole summary".into();
        let refs = vec![badge("a"), badge("b"), badge("◆ v1"), badge("◆ v2")];
        assert_eq!(
            row_hint(&c, Some(&refs), true),
            Some("◆ v1\n◆ v2\n\nwhole summary".into())
        );

        // A cut shown label comes first, in full, ahead of the folded ones.
        let long = "b".repeat(BADGE_COLS + 1);
        let refs = vec![badge(&long), badge("b"), badge("c")];
        assert_eq!(row_hint(&c, Some(&refs), false), Some(format!("{long}\nc")));
    }

    #[test]
    fn a_long_fold_is_capped_with_the_number_left_out() {
        let c = commits(1).remove(0);
        let refs: Vec<Badge> = (0..MAX_BADGES + HINT_MAX_REFS + 6)
            .map(|i| Badge {
                label: format!("◆ v{i}"),
                kind: badges::Kind::Tag,
            })
            .collect();
        let hint = row_hint(&c, Some(&refs), false).expect("hint");
        let lines: Vec<_> = hint.lines().collect();
        assert_eq!(lines.len(), HINT_MAX_REFS + 1);
        assert_eq!(lines[0], format!("◆ v{MAX_BADGES}"));
        assert_eq!(lines[HINT_MAX_REFS], "… and 6 more");
    }

    fn ids(out: Option<Output>) -> Vec<Oid> {
        match out {
            Some(Output::ShowRange(c) | Output::Batch(_, c)) => {
                c.into_iter().map(|c| c.id).collect()
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn shift_click_selects_a_range_in_list_order() {
        let list = commits(6);
        let mut s = State::default();
        let _ = s.update_list(Message::Select(list[4].id.clone()), &list);
        let out = s
            .update_list(Message::SelectRange(list[1].id.clone()), &list)
            .output;
        assert_eq!(ids(out), [1, 2, 3, 4].map(|i| list[i].id.clone()));
        // A plain click goes back to one commit.
        let _ = s.update_list(Message::Select(list[0].id.clone()), &list);
        assert!(s.marked.is_empty());
        // Without an anchor shift-click is a plain select.
        let mut s = State::default();
        let out = s
            .update_list(Message::SelectRange(list[2].id.clone()), &list)
            .output;
        assert!(matches!(out, Some(Output::ShowCommit(o)) if o == list[2].id));
    }

    #[test]
    fn command_click_toggles_and_batches_newest_first() {
        let list = commits(5);
        let mut s = State::default();
        let _ = s.update_list(Message::Select(list[3].id.clone()), &list);
        let _ = s.update_list(Message::ToggleMark(list[0].id.clone()), &list);
        let out = s
            .update_list(Message::ToggleMark(list[2].id.clone()), &list)
            .output;
        assert_eq!(ids(out), [0, 2, 3].map(|i| list[i].id.clone()));
        let out = s
            .update_list(Message::Batch(BatchAction::Revert), &list)
            .output;
        assert!(matches!(out, Some(Output::Batch(BatchAction::Revert, ref c)) if c.len() == 3));
        // Removing down to one falls back to a single selection.
        let _ = s.update_list(Message::ToggleMark(list[0].id.clone()), &list);
        let out = s
            .update_list(Message::ToggleMark(list[2].id.clone()), &list)
            .output;
        assert!(matches!(out, Some(Output::ShowCommit(o)) if o == list[3].id));
        assert!(s.marked.is_empty());
        assert!(
            s.update_list(Message::Batch(BatchAction::CherryPick), &list)
                .output
                .is_none()
        );
    }

    fn menu_title(out: Option<Output>) -> (Option<Oid>, String) {
        match out {
            Some(Output::Menu { show, menu }) => (show, menu.title),
            other => panic!("expected a menu, got {other:?}"),
        }
    }

    #[test]
    fn right_click_selects_and_opens_the_right_menu() {
        let list = commits(4);
        let mut s = State::default();
        // Unselected row: selects it, shows it, single-commit menu.
        let (show, title) = menu_title(
            s.update_list(Message::RightClick(list[1].id.clone()), &list)
                .output,
        );
        assert_eq!(show, Some(list[1].id.clone()));
        assert!(title.contains("c1"));
        // Same row again: nothing new to show.
        let (show, _) = menu_title(
            s.update_list(Message::RightClick(list[1].id.clone()), &list)
                .output,
        );
        assert!(show.is_none());
        // Inside a multi-selection: batch menu, selection kept.
        let _ = s.update_list(Message::SelectRange(list[3].id.clone()), &list);
        let (show, title) = menu_title(
            s.update_list(Message::RightClick(list[2].id.clone()), &list)
                .output,
        );
        assert!(show.is_none());
        assert_eq!(title, "3 commits selected");
        assert_eq!(s.marked.len(), 3);
        let (_, title) = menu_title(s.update_list(Message::OpenActions, &list).output);
        assert_eq!(title, "3 commits selected");
        // Outside it: back to one commit.
        let _ = s.update_list(Message::RightClick(list[0].id.clone()), &list);
        assert!(s.marked.is_empty());
        let (_, title) = menu_title(s.update_list(Message::OpenActions, &list).output);
        assert!(title.contains("c0"));
    }

    #[test]
    fn details_start_closed_and_toggle() {
        let mut s = State::default();
        assert!(!s.show_details);
        let _ = s.update(Message::ToggleDetails);
        assert!(s.show_details);
        // Stays open when another commit is selected.
        let _ = s.update(Message::Select("c".repeat(40).parse().expect("oid")));
        assert!(s.show_details);
    }

    #[test]
    fn arrows_move_and_shift_arrows_extend_from_the_cursor() {
        let list = commits(5);
        let mut s = State::default();
        let key = |k| Nav::plain(k);
        let msg = |s: &State, n: Nav| s.nav_in(n, &list, 10.0).and_then(|m| m.message);
        // Nothing selected: ↓ starts at the newest commit.
        assert!(matches!(msg(&s, key(Key::Down)), Some(Message::Select(id)) if id == list[0].id));
        let _ = s.update_list(Message::Select(list[1].id.clone()), &list);
        assert!(matches!(msg(&s, key(Key::Down)), Some(Message::Select(id)) if id == list[2].id));
        let shift = Nav {
            key: Key::Down,
            shift: true,
        };
        let Some(Message::SelectRange(to)) = msg(&s, shift) else {
            panic!("shift-↓ extends");
        };
        let _ = s.update_list(Message::SelectRange(to), &list);
        // The next shift-↓ continues from the moving end, not the anchor.
        assert!(matches!(msg(&s, shift), Some(Message::SelectRange(id)) if id == list[3].id));
        assert!(matches!(
            msg(&s, key(Key::Char('.'))),
            Some(Message::OpenActions)
        ));
        assert!(s.nav_in(key(Key::Right), &list, 10.0).is_none());
    }
}
