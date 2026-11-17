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

use std::collections::HashMap;
use std::sync::Arc;

use gf_core::model::{Commit, Oid, RefKind};
use gf_git::graph::GraphRow;
use iced::widget::text::Wrapping;
use iced::widget::{Row, column, container, row};
use iced::{Color, Element, Length, Padding, Task};

use crate::feature::{Cx, Feature, Update};
use crate::features::context_menu::{Entry, Menu};
use crate::nav::{self, Key, Moved, Nav};
use crate::repo_tab::{HISTORY_PAGE, History, Snapshot};
use crate::theme::{Tokens, style};
use crate::widgets::graph_canvas;
use crate::widgets::memo::{self, Memo};
use crate::widgets::virtual_list::{ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim, format, mono};

/// Scrollable id of the commit list.
pub const LIST_ID: &str = "gf-history-list";
/// Ref badges shown per commit before collapsing into `+N`.
const MAX_BADGES: usize = 2;
/// Start loading the next page this close (px) to the end of the list.
const NEAR_END_PX: f32 = 200.0;

#[derive(Debug, Default)]
pub struct State {
    /// Revision being shown; `None` = the snapshot's history.
    revision: Option<String>,
    /// History for `revision`, loaded by the app.
    data: Option<Arc<History>>,
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
    /// Lanes and widest summary of the history shown (see `view`).
    widths: Memo<(memo::Id, memo::Id), (usize, usize)>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(Oid),
    Scrolled(ScrollPos),
    ToggleAllBranches,
    BranchHere,
    TagHere,
    Checkout,
    CopyId,
    BackToHead,
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
    ShowCommit(Oid),
    ToggleAllBranches,
    NewBranchAt(Oid),
    NewTagAt(Oid),
    CheckoutDetached(Oid),
    CopyId(Oid),
    BackToHead,
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
        self.loading = revision.is_some();
        self.revision = revision;
        self.data = None;
        self.scroll = ScrollPos::default();
        self.revision_limit = 0;
        self.loading_more = false;
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

    /// Range/toggle selection needs the list being shown.
    pub fn update_with(
        &mut self,
        message: Message,
        snap: Option<&Snapshot>,
    ) -> Update<Message, Output> {
        let (commits, more): (Vec<Commit>, bool) = match snap.and_then(|s| self.history(s)) {
            Some(h) => (h.commits.clone(), h.more),
            None => (Vec::new(), false),
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
        let mut up = self.update_list(message, &commits);
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
                if self.marked.is_empty() {
                    if let Some(sel) = self.selected.clone() {
                        self.marked.push(sel);
                    }
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
                    menu: commit_menu(c),
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
                        menu: commit_menu(c),
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
    }

    fn history<'a>(&'a self, snap: &'a Snapshot) -> Option<&'a History> {
        match (&self.revision, &self.data) {
            (None, _) => Some(&snap.history),
            (Some(_), Some(d)) => Some(d),
            (Some(_), None) => None,
        }
    }
}

/// `limit`, or one page when unset.
fn page_or(limit: usize) -> usize {
    if limit == 0 { HISTORY_PAGE } else { limit }
}

/// Row height for the list: one line of text plus padding.
fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.7).round()
}

/// `oid → [(label, color)]` for branch/tag badges, HEAD first.
fn decorations<'a>(t: &Tokens, snap: &'a Snapshot) -> HashMap<&'a Oid, Vec<(String, Color)>> {
    let mut map: HashMap<&Oid, Vec<(String, Color)>> = HashMap::new();
    for b in &snap.branches {
        let (label, color) = match b.name.kind() {
            RefKind::LocalBranch if b.is_head => (format!("HEAD → {}", b.name.short()), t.head()),
            RefKind::LocalBranch => (b.name.short().to_owned(), t.branch()),
            _ => (b.name.short().to_owned(), t.remote()),
        };
        let list = map.entry(&b.target).or_default();
        if b.is_head {
            list.insert(0, (label, color));
        } else {
            list.push((label, color));
        }
    }
    for tag in &snap.tags {
        map.entry(&tag.target)
            .or_default()
            .push((format!("◆ {}", tag.name.short()), t.tag()));
    }
    if snap.status.head.branch.is_none() {
        if let Some(oid) = &snap.status.head.oid {
            map.entry(oid)
                .or_default()
                .insert(0, ("HEAD".into(), t.head()));
        }
    }
    map
}

/// Columns reserved for the author and date.
const AUTHOR_COLS: usize = 16;
const DATE_COLS: usize = 16;
/// Badges + summary are clipped to at most this many columns so author and
/// date stay aligned; the full text is in the detail box.
const MAX_SUMMARY_COLS: usize = 110;

/// Badges and summary as one text line, plus its width in columns.
fn summary_line(
    c: &Commit,
    badges: Option<&Vec<(String, Color)>>,
) -> (Vec<(String, Option<Color>)>, usize) {
    let mut parts: Vec<(String, Option<Color>)> = Vec::new();
    if let Some(refs) = badges {
        for (label, color) in refs.iter().take(MAX_BADGES) {
            parts.push((format!("({}) ", format::truncate(label, 28)), Some(*color)));
        }
        if refs.len() > MAX_BADGES {
            parts.push((format!("+{} ", refs.len() - MAX_BADGES), None));
        }
    }
    parts.push((c.summary.clone(), None));
    let cols = parts.iter().map(|(s, _)| columns(s)).sum();
    (parts, cols)
}

#[allow(clippy::too_many_arguments)]
fn commit_row<'a>(
    t: &'a Tokens,
    c: &'a Commit,
    g: Option<&'a GraphRow>,
    graph_px: f32,
    summary_px: f32,
    badges: Option<&Vec<(String, Color)>>,
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

    let (parts, _) = summary_line(c, badges);
    let mut summary = Row::new();
    for (text, color) in parts {
        summary = summary.push(match color {
            Some(color) => colored(t, text, color).wrapping(Wrapping::None),
            None => mono(t, text).wrapping(Wrapping::None),
        });
    }
    line = line
        .push(
            container(summary)
                .width(Length::Fixed(summary_px))
                .clip(true),
        )
        .push(
            container(
                dim(t, format::truncate(&c.author.name, AUTHOR_COLS)).wrapping(Wrapping::None),
            )
            .width(Length::Fixed(t.cols(AUTHOR_COLS))),
        )
        .push(dim(t, format::datetime(c.author.time)).wrapping(Wrapping::None));
    let row: Element<'a, Message> = iced::widget::mouse_area(widgets::list_row_flush(
        t,
        line,
        selected,
        Message::Select(c.id.clone()),
    ))
    .on_right_press(Message::RightClick(c.id.clone()))
    .into();
    match badges {
        Some(refs) if refs.len() > MAX_BADGES => {
            let all: Vec<&str> = refs.iter().map(|(l, _)| l.as_str()).collect();
            widgets::tip(t, row, all.join(", "))
        }
        _ => row,
    }
}

/// `[ actions ▾ ]`: opens the same menu as right-clicking the selection.
fn actions_button(t: &Tokens) -> Element<'_, Message> {
    widgets::tip(
        t,
        widgets::text_button(t, "actions ▾", None, Some(Message::OpenActions)),
        "all actions for the selection · or right-click a commit".into(),
    )
}

/// Actions for one commit.
fn commit_menu(c: &Commit) -> Menu<Message> {
    let act = |a| Message::Act(a);
    Menu {
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
    }
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
/// toggle; with details open, also author, date and the full message.
fn detail<'a>(t: &'a Tokens, c: &'a Commit, open: bool) -> Element<'a, Message> {
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
            Message::Act(action) => {
                selected.map_or_else(Update::none, |o| Update::output(Output::Act(action, o)))
            }
            Message::ToggleDetails => {
                self.show_details = !self.show_details;
                Update::none()
            }
        }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let all = cx.all_branches;
        let scope = match &self.revision {
            Some(r) => r.rsplit('/').next().unwrap_or(r).to_owned(),
            None if all => "all branches".into(),
            None => "HEAD".into(),
        };
        let mut toolbar = row![dim(t, format!("showing: {scope}")).width(Length::Fill)]
            .spacing(4)
            .align_y(iced::Alignment::Center);
        if self.revision.is_some() {
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

        let Some(snap) = cx.snapshot else {
            return widgets::panel(t, "HISTORY".into(), widgets::empty_hint(t, "loading…"));
        };
        if self.loading {
            return widgets::panel(
                t,
                "HISTORY".into(),
                column![toolbar, widgets::empty_hint(t, "loading history…")],
            );
        }
        let Some(history) = self.history(snap) else {
            return widgets::panel(t, "HISTORY".into(), toolbar);
        };
        if history.commits.is_empty() {
            return widgets::panel(
                t,
                "HISTORY".into(),
                column![
                    toolbar,
                    widgets::empty_hint(
                        t,
                        "no commits yet — open the working tree (⌘E) to stage and commit"
                    )
                ],
            );
        }

        let decor = decorations(t, snap);
        // Walks every commit: once per history / refs change, not per frame.
        let key = (
            memo::identity(history, history.commits.len()),
            memo::identity(snap, snap.branches.len() + snap.tags.len()),
        );
        let (lanes, summary_cols) = self.widths.get(key, || {
            let lanes = history.graph.iter().map(|g| g.width).max().unwrap_or(1);
            let summary = history
                .commits
                .iter()
                .map(|c| summary_line(c, decor.get(&c.id)).1)
                .max()
                .unwrap_or(0)
                .clamp(20, MAX_SUMMARY_COLS);
            (lanes, summary)
        });
        let graph_px = graph_canvas::width_for(lanes);
        let summary_px = t.cols(summary_cols);
        // graph, hash, summary, author, date + gaps and padding.
        let content_width =
            graph_px + t.cols(7 + AUTHOR_COLS + DATE_COLS) + summary_px + 4.0 * 6.0 + 12.0;
        let selected = self.selected.as_ref();
        let marked = &self.marked;
        let is_selected = move |id: &Oid| {
            if marked.is_empty() {
                selected == Some(id)
            } else {
                marked.contains(id)
            }
        };
        let list = virtual_list(
            LIST_ID,
            history.commits.len(),
            row_height(t),
            content_width,
            self.scroll,
            move |i| {
                let c = &history.commits[i];
                commit_row(
                    t,
                    c,
                    history.graph.get(i),
                    graph_px,
                    summary_px,
                    decor.get(&c.id),
                    is_selected(&c.id),
                )
            },
            Message::Scrolled,
        );

        let mut content = column![toolbar, list].spacing(4);
        if self.marked.len() > 1 {
            content = content.push(batch_bar(t, &self.marked_commits(&history.commits)));
        } else if let Some(c) = self
            .selected
            .as_ref()
            .and_then(|s| history.commits.iter().find(|c| &c.id == s))
        {
            content = content.push(detail(t, c, self.show_details));
        }
        let count = history.commits.len();
        let more = if history.more { "+" } else { "" };
        widgets::panel(t, format!("HISTORY ({count}{more})"), content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
