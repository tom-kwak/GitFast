//! Sidebar: local branches, remote branches, tags and stashes as a
//! collapsible tree with a quick filter. Selecting an item shows its
//! history (or diff, for stashes) and an action bar for it; section headers
//! have `+` buttons to create new items.

use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap, HashSet};

use gf_core::model::{Branch, RefKind, StashEntry, Tag};
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, space, text_input};
use iced::{Element, Length, Padding};

use crate::feature::{Cx, Feature, Update};
use crate::features::context_menu::{Entry, Menu};
use crate::nav::{self, Key, Moved, Nav};
use crate::repo_tab::Snapshot;
use crate::theme::{Tokens, style};
use crate::widgets::virtual_list::{ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim, format};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Branches,
    Remotes,
    Tags,
    Stashes,
    Submodules,
    Lfs,
}

/// What is selected in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Full ref name of a branch or tag.
    Ref(String),
    Stash(usize),
    Submodule(std::path::PathBuf),
    LfsPattern(String),
}

#[derive(Debug)]
pub struct State {
    layout: OnceCell<(Vec<SideRow>, usize)>,
    collapsed: HashSet<Section>,
    filter: String,
    selected: Option<Selection>,
    marked_branches: BTreeSet<String>,
    mark_cursor: Option<String>,
    mark_anchor: Option<String>,
    /// Branch folders (`feature/`) the user closed, by section and path.
    folded: HashSet<(Section, String)>,
    /// The keyboard cursor is on this section header or folder (instead of
    /// the selected item).
    cursor: Option<Fold>,
    /// The last item clicked and when, to recognize a double click (iced
    /// cannot: the row's button consumes the press).
    last_click: Option<(Selection, std::time::Instant)>,
    scroll: ScrollPos,
}

impl Default for State {
    fn default() -> Self {
        Self {
            layout: OnceCell::new(),
            // Only branches are open at first; the other sections are a
            // click (or →) away and keep the list short.
            collapsed: HashSet::from([
                Section::Remotes,
                Section::Tags,
                Section::Stashes,
                Section::Submodules,
                Section::Lfs,
            ]),
            filter: String::new(),
            selected: None,
            marked_branches: BTreeSet::new(),
            mark_cursor: None,
            mark_anchor: None,
            folded: HashSet::new(),
            cursor: None,
            last_click: None,
            scroll: ScrollPos::default(),
        }
    }
}

/// Two clicks on the same item within this time are a double click.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

/// Something that opens and closes: a section or a branch folder.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Fold {
    Section(Section),
    /// Section and folder path (`feature`, `origin/feature`).
    Folder(Section, String),
}

/// Rows the keyboard can stop on.
#[derive(Debug, Clone)]
enum Item {
    Fold(Fold),
    /// An item and what Enter does with it.
    Pick(Selection, Option<Action>),
}

impl Item {
    fn of(row: &SideRow) -> Option<Self> {
        Some(match row {
            SideRow::Header(sec, ..) => Self::Fold(Fold::Section(*sec)),
            SideRow::Folder { section, path, .. } => {
                Self::Fold(Fold::Folder(*section, (*path).to_owned()))
            }
            SideRow::Branch(b, _) => Self::Pick(
                Selection::Ref(b.name.as_str().to_owned()),
                (!b.is_head).then_some(Action::Checkout),
            ),
            SideRow::Tag(tg) => Self::Pick(Selection::Ref(tg.name.as_str().to_owned()), None),
            SideRow::Stash(st) => Self::Pick(Selection::Stash(st.index), None),
            SideRow::Submodule(m) => {
                Self::Pick(Selection::Submodule(m.path.clone()), Some(Action::Open))
            }
            SideRow::Lfs(p) => Self::Pick(Selection::LfsPattern((*p).clone()), None),
            SideRow::Hint(_) | SideRow::UnbornBranch(_) => return None,
        })
    }
}

/// Nesting level of a row: sections are 0, their rows 1, and each branch
/// folder adds one.
fn depth(row: &SideRow) -> usize {
    match row {
        SideRow::Header(..) => 0,
        SideRow::Folder { depth, .. } | SideRow::Branch(_, depth) => depth + 1,
        _ => 1,
    }
}

/// Branches of one section as a tree of `/` folders: `feature/a` and
/// `feature/b` are listed under a `feature/` folder (remote branches under
/// their remote's folder first). `folded` folders hide their contents;
/// while filtering everything that matches is shown.
fn branch_tree<'a>(
    section: Section,
    branches: &[&'a Branch],
    folded: impl Fn(&str) -> bool,
    filtering: bool,
) -> Vec<SideRow> {
    let mut sorted: Vec<&'a Branch> = branches.to_vec();
    sorted.sort_by(|a, b| a.name.short().cmp(b.name.short()));
    // Folder prefixes of a name: `a/b/c` → [`a`, `a/b`].
    let prefixes = |name: &'a str| -> Vec<&'a str> {
        name.match_indices('/')
            .map(|(i, _)| &name[..i])
            .filter(|p| !p.is_empty())
            .collect()
    };
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for b in &sorted {
        for p in prefixes(b.name.short()) {
            *counts.entry(p).or_default() += 1;
        }
    }
    let mut rows = Vec::new();
    // Folders currently open in the listing, outermost first.
    let mut open: Vec<&str> = Vec::new();
    for b in sorted {
        let dirs = prefixes(b.name.short());
        let mut hidden = false;
        for (d, dir) in dirs.iter().enumerate() {
            if open.get(d) != Some(dir) {
                open.truncate(d);
                open.push(dir);
                rows.push(SideRow::Folder {
                    section,
                    path: (*dir).to_owned(),
                    depth: d,
                    count: counts.get(dir).copied().unwrap_or(0),
                });
            }
            if !filtering && folded(dir) {
                hidden = true;
                open.truncate(d + 1);
                break;
            }
        }
        if !hidden {
            open.truncate(dirs.len());
            rows.push(SideRow::Branch(b.clone(), dirs.len()));
        }
    }
    rows
}

/// `refs/heads/main` → `main`.
fn short_name(full: &str) -> &str {
    ["refs/heads/", "refs/remotes/", "refs/tags/"]
        .iter()
        .find_map(|p| full.strip_prefix(p))
        .unwrap_or(full)
}

/// The filter field (↑/↓ in it walk the filtered tree).
pub const REFS_FILTER_ID: &str = "gf-refs-filter";
/// Scrollable id of the tree.
const LIST_ID: &str = "gf-refs";

fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.8).round()
}

/// Gap between a header's `▾` and its label.
const ARROW_GAP: f32 = 6.0;

/// Lead-in of a row under a section header: past the header's `▾` and gap,
/// so items line up with the label and each section reads as one block;
/// folders nest two columns further per level.
fn indent(t: &Tokens, depth: usize) -> iced::widget::Space {
    space().width(Length::Fixed(t.cols(1 + depth * 2) + ARROW_GAP))
}

/// One row of the flattened tree.
#[derive(Debug)]
enum SideRow {
    Header(Section, &'static str, usize, Option<(Action, &'static str)>),
    /// A `/` folder of branches: its path, nesting and branch count.
    Folder {
        section: Section,
        path: String,
        depth: usize,
        count: usize,
    },
    /// A branch and how many folders it is nested in.
    Branch(Branch, usize),
    Tag(Tag),
    Stash(StashEntry),
    Submodule(gf_git::Submodule),
    Lfs(String),
    Hint(&'static str),
    UnbornBranch(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    Toggle(Section),
    /// Open/close a branch folder.
    ToggleFolder(Section, String),
    Filter(String),
    Select(Selection),
    ToggleBranch(String),
    SelectBranchRange(String),
    ClearBranchMarks,
    /// Header `+` buttons and the actions menu entries.
    Act(Action),
    /// `.` on the keyboard: menu for the selection.
    OpenActions,
    /// Right-click on an item: select it and open its menu.
    RightClick(Selection),
    Scrolled(ScrollPos),
    /// The section's `«` button: hide the sidebar.
    Collapse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NewBranch,
    BranchHere,
    NewTag,
    NewStash,
    Checkout,
    Rename,
    Delete,
    Push,
    StashApply,
    StashPop,
    /// Merge the selected branch into the current one.
    Merge,
    /// Rebase the current branch onto the selected one.
    RebaseOnto,
    /// Show the diff between the current branch and the selected one.
    Compare,
    /// Check the selected branch out in a new worktree.
    NewWorktree,
    /// Open the selected submodule in a new tab.
    Open,
    UpdateSubmodules,
    TrackLfs,
    LfsPull,
}

impl Action {
    /// The section a create action adds its item to.
    fn creates_in(self) -> Option<Section> {
        match self {
            Self::NewBranch => Some(Section::Branches),
            Self::NewTag => Some(Section::Tags),
            Self::NewStash => Some(Section::Stashes),
            Self::TrackLfs => Some(Section::Lfs),
            _ => None,
        }
    }
}

/// Requests to the app. Destructive ones are confirmed by the app.
#[derive(Debug, Clone)]
pub enum Output {
    /// Show history of this ref; `None` = back to the default history.
    ShowRevision(Option<String>),
    ShowStash(StashEntry),
    NewBranch {
        from: Option<String>,
        branch_here: bool,
    },
    NewTag,
    NewStash,
    Checkout(Branch),
    RenameBranch(String),
    DeleteBranches(Vec<Branch>),
    PushBranch(Branch),
    /// Merge this branch (short name) into the current branch.
    Merge(String),
    /// Rebase the current branch onto this branch (short name).
    RebaseOnto(String),
    /// Diff the checked-out commit against this branch.
    Compare(Branch),
    /// Ask for a new worktree with this (local) branch checked out.
    NewWorktree(String),
    /// Open a repository folder (a submodule) in a new tab.
    OpenRepo(std::path::PathBuf),
    UpdateSubmodules,
    TrackLfs,
    UntrackLfs(String),
    LfsPull,
    DeleteTag(String),
    PushTag(String),
    StashApply(usize),
    StashPop(usize),
    StashDrop(usize),
    /// Hide the sidebar (the app keeps a strip to bring it back).
    Collapse,
    /// Open the actions menu; `show` is a newly selected item whose
    /// history/diff the app should show first.
    Menu {
        show: Option<Selection>,
        menu: Menu<Message>,
    },
}

impl State {
    /// Forgets a selection that no longer exists (deleted branch, dropped
    /// stash). Returns false when it was forgotten.
    pub fn retain_existing(&mut self, snapshot: &Snapshot) -> bool {
        self.layout.take();
        if !self.marked_branches.is_empty() {
            let existing: HashSet<_> = snapshot.branches.iter().map(|b| b.name.as_str()).collect();
            self.marked_branches
                .retain(|name| existing.contains(name.as_str()));
            if self
                .mark_cursor
                .as_ref()
                .is_some_and(|name| !existing.contains(name.as_str()))
                || self.marked_branches.is_empty()
            {
                self.mark_cursor = None;
            }
            if self.marked_branches.is_empty()
                || self
                    .mark_anchor
                    .as_ref()
                    .is_some_and(|name| !existing.contains(name.as_str()))
            {
                self.mark_anchor = None;
            }
        }
        let exists = match &self.selected {
            None => return true,
            Some(Selection::Ref(r)) => {
                snapshot.branches.iter().any(|b| b.name.as_str() == r)
                    || snapshot.tags.iter().any(|t| t.name.as_str() == r)
            }
            Some(Selection::Stash(i)) => *i < snapshot.stashes.len(),
            Some(Selection::Submodule(p)) => snapshot.submodules.iter().any(|m| &m.path == p),
            Some(Selection::LfsPattern(pat)) => {
                snapshot.lfs.as_ref().is_some_and(|l| l.contains(pat))
            }
        };
        if !exists {
            self.selected = None;
        }
        exists
    }

    pub fn clear_selection(&mut self) {
        self.clear_branch_marks();
        self.selected = None;
    }

    /// Select and reveal the local branch that already has the remote checked out.
    pub fn select_local_branch(
        &mut self,
        branch: &Branch,
        snap: &Snapshot,
        t: &Tokens,
    ) -> iced::Task<Message> {
        self.clear_branch_marks();
        self.layout.take();
        let full = branch.name.as_str();
        let short = branch.name.short();
        self.selected = Some(Selection::Ref(full.to_owned()));
        self.cursor = None;
        self.last_click = None;
        self.collapsed.remove(&Section::Branches);
        if !self.matches(short) {
            self.filter.clear();
        }
        for (slash, _) in short.match_indices('/') {
            self.folded
                .remove(&(Section::Branches, short[..slash].to_owned()));
        }
        self.layout(snap)
            .0
            .iter()
            .position(|row| matches!(row, SideRow::Branch(b, _) if b.name.as_str() == full))
            .map_or_else(iced::Task::none, |row| {
                nav::reveal(LIST_ID, self.scroll, row, row_height(t))
            })
    }

    fn matches(&self, name: &str) -> bool {
        self.filter.is_empty() || name.to_lowercase().contains(&self.filter.to_lowercase())
    }

    fn is_selected_ref(&self, full: &str) -> bool {
        if self.marked_branches.is_empty() {
            matches!(&self.selected, Some(Selection::Ref(r)) if r == full)
        } else {
            self.marked_branches.contains(full)
        }
    }

    fn clear_branch_marks(&mut self) {
        self.marked_branches.clear();
        self.mark_cursor = None;
        self.mark_anchor = None;
    }

    fn mark_branch(&mut self, name: String, range: bool, snap: &Snapshot) {
        if !snap.branches.iter().any(|b| b.name.as_str() == name) {
            return;
        }
        let selected = match &self.selected {
            Some(Selection::Ref(anchor))
                if snap.branches.iter().any(|b| b.name.as_str() == anchor) =>
            {
                Some(anchor.as_str())
            }
            _ => None,
        };
        let anchor = self.mark_anchor.as_deref().or(selected).unwrap_or(&name);
        if range {
            let rows = &self.layout(snap).0;
            let branches: Vec<_> = rows
                .iter()
                .filter_map(|row| match row {
                    SideRow::Branch(b, _) => Some(b.name.as_str()),
                    _ => None,
                })
                .collect();
            let index = |r: &str| branches.iter().position(|&b| b == r);
            self.marked_branches = index(&name).map_or_else(
                || BTreeSet::from([name.clone()]),
                |end| {
                    let start = index(anchor).unwrap_or(end);
                    branches[start.min(end)..=start.max(end)]
                        .iter()
                        .map(|&r| r.to_owned())
                        .collect()
                },
            );
        } else {
            if self.marked_branches.is_empty()
                && let Some(anchor) = selected
            {
                self.marked_branches.insert(anchor.to_owned());
            }
            if !self.marked_branches.remove(&name) {
                self.marked_branches.insert(name.clone());
            }
        }
        self.mark_anchor = (!self.marked_branches.is_empty()).then(|| anchor.to_owned());
        self.mark_cursor = (!self.marked_branches.is_empty()).then_some(name);
        self.cursor = None;
        self.last_click = None;
    }

    fn tree(&self, section: Section, branches: &[&Branch]) -> Vec<SideRow> {
        branch_tree(
            section,
            branches,
            |p| self.folded.contains(&(section, p.to_owned())),
            !self.filter.is_empty(),
        )
    }

    fn is_folded(&self, fold: &Fold) -> bool {
        match fold {
            Fold::Section(sec) => self.collapsed.contains(sec),
            Fold::Folder(sec, p) => self.folded.contains(&(*sec, p.clone())),
        }
    }

    fn set_folded(&mut self, fold: &Fold, folded: bool) {
        self.layout.take();
        match (fold, folded) {
            (Fold::Section(sec), true) => {
                self.collapsed.insert(*sec);
            }
            (Fold::Section(sec), false) => {
                self.collapsed.remove(sec);
            }
            (Fold::Folder(sec, p), true) => {
                self.folded.insert((*sec, p.clone()));
            }
            (Fold::Folder(sec, p), false) => {
                self.folded.remove(&(*sec, p.clone()));
            }
        }
    }

    fn toggle_message(fold: &Fold) -> Message {
        match fold {
            Fold::Section(sec) => Message::Toggle(*sec),
            Fold::Folder(sec, p) => Message::ToggleFolder(*sec, p.clone()),
        }
    }

    fn layout(&self, snap: &Snapshot) -> &(Vec<SideRow>, usize) {
        self.layout.get_or_init(|| {
            let rows = self.rows(snap);
            let name_cols = rows
                .iter()
                .map(|r| match r {
                    SideRow::Branch(b, d) => columns(b.name.short()) + 12 + d * 2,
                    SideRow::Folder { path, depth, .. } => columns(path) + 12 + depth * 2,
                    SideRow::Tag(tg) => columns(tg.name.short()),
                    SideRow::Stash(s) => columns(&s.message),
                    SideRow::Submodule(m) => columns(&m.path.display().to_string()) + 24,
                    SideRow::Lfs(p) => columns(p),
                    SideRow::Hint(h) => columns(h),
                    SideRow::UnbornBranch(name) => columns(name) + columns("●  (no commits yet)"),
                    SideRow::Header(..) => 16,
                })
                .max()
                .unwrap_or(0)
                // Every row under a header is indented (`indent`).
                + 2;
            (rows, name_cols)
        })
    }

    /// The flattened tree as shown.
    fn rows(&self, snap: &Snapshot) -> Vec<SideRow> {
        let locals: Vec<_> = snap
            .branches
            .iter()
            .filter(|b| b.name.kind() == RefKind::LocalBranch && self.matches(b.name.short()))
            .collect();
        let remotes: Vec<_> = snap
            .branches
            .iter()
            .filter(|b| b.name.kind() == RefKind::RemoteBranch && self.matches(b.name.short()))
            .collect();
        let tags: Vec<_> = snap
            .tags
            .iter()
            .filter(|tg| self.matches(tg.name.short()))
            .collect();
        let stashes: Vec<_> = snap
            .stashes
            .iter()
            .filter(|s| self.matches(&s.message))
            .collect();
        let mut rows: Vec<SideRow> = Vec::new();
        let open = |sec: Section| !self.collapsed.contains(&sec);

        let head = &snap.status.head;
        let unborn = head
            .branch
            .as_deref()
            .filter(|name| head.oid.is_none() && self.matches(name));
        rows.push(SideRow::Header(
            Section::Branches,
            "BRANCHES",
            locals.len() + usize::from(unborn.is_some()),
            Some((Action::NewBranch, "new branch…")),
        ));
        if open(Section::Branches) {
            if let Some(name) = unborn {
                rows.push(SideRow::UnbornBranch(name.to_owned()));
            } else if locals.is_empty() {
                rows.push(SideRow::Hint(if self.filter.is_empty() {
                    "none yet — make a first commit"
                } else {
                    "no match"
                }));
            }
            rows.extend(self.tree(Section::Branches, &locals));
        }
        rows.push(SideRow::Header(
            Section::Remotes,
            "REMOTES",
            remotes.len(),
            None,
        ));
        if open(Section::Remotes) {
            if remotes.is_empty() {
                rows.push(SideRow::Hint(if snap.remotes.is_empty() {
                    "none — `remote add <name> <url>`"
                } else {
                    "none fetched — try fetch"
                }));
            }
            rows.extend(self.tree(Section::Remotes, &remotes));
        }
        rows.push(SideRow::Header(
            Section::Tags,
            "TAGS",
            tags.len(),
            Some((Action::NewTag, "new tag on HEAD…")),
        ));
        if open(Section::Tags) {
            if tags.is_empty() {
                rows.push(SideRow::Hint("none — + creates one"));
            }
            rows.extend(tags.into_iter().cloned().map(SideRow::Tag));
        }
        rows.push(SideRow::Header(
            Section::Stashes,
            "STASHES",
            stashes.len(),
            Some((Action::NewStash, "stash changes…")),
        ));
        if open(Section::Stashes) {
            if stashes.is_empty() {
                rows.push(SideRow::Hint("none — + puts changes aside"));
            }
            rows.extend(stashes.into_iter().cloned().map(SideRow::Stash));
        }

        // Optional sections: only shown when the repository has them.
        // Worktrees have their own view (`REFS │ WORKTREES` above this).
        if !snap.submodules.is_empty() {
            let subs: Vec<_> = snap
                .submodules
                .iter()
                .filter(|m| self.matches(&m.path.display().to_string()))
                .collect();
            rows.push(SideRow::Header(
                Section::Submodules,
                "SUBMODULES",
                subs.len(),
                None,
            ));
            if open(Section::Submodules) {
                rows.extend(subs.into_iter().cloned().map(SideRow::Submodule));
            }
        }
        if let Some(patterns) = &snap.lfs {
            let pats: Vec<_> = patterns.iter().filter(|p| self.matches(p)).collect();
            rows.push(SideRow::Header(
                Section::Lfs,
                "LFS",
                pats.len(),
                Some((Action::TrackLfs, "track a file pattern with LFS…")),
            ));
            if open(Section::Lfs) {
                rows.extend(pats.into_iter().cloned().map(SideRow::Lfs));
            }
        }
        rows
    }

    /// Keys for the tree: arrows move over headers, folders and items (an
    /// item is selected as the cursor reaches it), ←/→ fold and unfold,
    /// Enter/Space fold a header or folder or check out / open the item.
    pub fn nav(&mut self, nav: Nav, snap: Option<&Snapshot>, t: &Tokens) -> Option<Moved<Message>> {
        let snap = snap?;
        let items: Vec<(usize, Item, usize)> = self
            .layout(snap)
            .0
            .iter()
            .enumerate()
            .filter_map(|(i, r)| Item::of(r).map(|it| (i, it, depth(r))))
            .collect();
        let current = items.iter().position(|(_, it, _)| match it {
            Item::Fold(f) => self.cursor.as_ref() == Some(f),
            Item::Pick(sel, _) => {
                self.cursor.is_none()
                    && self.mark_cursor.as_ref().map_or_else(
                        || self.selected.as_ref() == Some(sel),
                        |name| matches!(sel, Selection::Ref(r) if r == name),
                    )
            }
        });
        let here = current.map(|c| items[c].1.clone());
        // The section or folder that contains the cursor's row.
        let parent = current.and_then(|c| {
            let d = items[c].2;
            items[..c].iter().rev().find_map(|(_, it, pd)| match it {
                Item::Fold(f) if *pd < d => Some(f.clone()),
                _ => None,
            })
        });
        match nav.key {
            Key::Left => {
                // An open section/folder closes; otherwise the cursor moves
                // to the one containing it, which closes.
                let fold = match &here {
                    Some(Item::Fold(f)) if !self.is_folded(f) => f.clone(),
                    _ => parent?,
                };
                self.set_folded(&fold, true);
                self.cursor = Some(fold);
                return Some(Moved::stay());
            }
            Key::Right => {
                return match here {
                    Some(Item::Fold(f)) if self.is_folded(&f) => {
                        self.set_folded(&f, false);
                        Some(Moved::stay())
                    }
                    _ => None,
                };
            }
            Key::Char('.') => {
                return (self.selected.is_some() || !self.marked_branches.is_empty())
                    .then(|| Moved::message(Message::OpenActions));
            }
            Key::Enter | Key::Space => {
                if !self.marked_branches.is_empty() {
                    return Some(Moved::stay());
                }
                return match here? {
                    Item::Fold(f) => Some(Moved::message(Self::toggle_message(&f))),
                    Item::Pick(_, Some(action)) => Some(Moved::message(Message::Act(action))),
                    Item::Pick(_, None) => None,
                };
            }
            _ => {}
        }
        let h = row_height(t);
        let i = nav::step(current, items.len(), nav.key, nav::page(self.scroll, h))?;
        if current == Some(i) {
            return Some(Moved::stay());
        }
        let (row, item, _) = if nav.shift {
            let range = if current.is_some_and(|c| c > i) {
                items[..=i].iter().rev().find(|(_, item, _)| {
                    matches!(item, Item::Pick(Selection::Ref(r), _) if r.starts_with("refs/heads/") || r.starts_with("refs/remotes/"))
                })
            } else {
                items[i..].iter().find(|(_, item, _)| {
                    matches!(item, Item::Pick(Selection::Ref(r), _) if r.starts_with("refs/heads/") || r.starts_with("refs/remotes/"))
                })
            };
            let Some((row, Item::Pick(Selection::Ref(name), _), _)) = range else {
                return Some(Moved::stay());
            };
            return Some(
                Moved::message(Message::SelectBranchRange(name.clone())).with_task(nav::reveal(
                    LIST_ID,
                    self.scroll,
                    *row,
                    h,
                )),
            );
        } else {
            items[i].clone()
        };
        let reveal = nav::reveal(LIST_ID, self.scroll, row, h);
        Some(match item {
            Item::Fold(f) => {
                self.cursor = Some(f);
                Moved::stay().with_task(reveal)
            }
            Item::Pick(sel, _) => Moved::message(Message::Select(sel)).with_task(reveal),
        })
    }

    fn header<'a>(
        &self,
        t: &'a Tokens,
        section: Section,
        label: &str,
        count: usize,
        add: Option<(Action, &'static str)>,
    ) -> Element<'a, Message> {
        let arrow = if self.collapsed.contains(&section) {
            "▸"
        } else {
            "▾"
        };
        let toggle =
            button(row![dim(t, arrow), dim(t, format!("{label} ({count})"))].spacing(ARROW_GAP))
                .padding(Padding::from([1, 6]))
                .style(style::row_button(
                    t,
                    self.cursor == Some(Fold::Section(section)),
                ))
                .on_press(Message::Toggle(section));
        match add {
            Some((action, hint)) => row![
                toggle,
                widgets::icon_button(t, "+", t.accent(), hint, Message::Act(action))
            ]
            .align_y(iced::Alignment::Center)
            .into(),
            None => toggle.into(),
        }
    }

    /// `▾ feature/ (3)`: opens and closes the folder.
    fn folder_row<'a>(
        &self,
        t: &'a Tokens,
        section: Section,
        path: &'a str,
        depth: usize,
        count: usize,
    ) -> Element<'a, Message> {
        let fold = Fold::Folder(section, path.to_owned());
        let arrow = if self.is_folded(&fold) { "▸" } else { "▾" };
        let name = path.rsplit('/').next().unwrap_or(path);
        let line = row![
            indent(t, depth),
            dim(t, arrow),
            colored(t, format!("{name}/"), t.fg).wrapping(Wrapping::None),
            dim(t, format!("({count})")),
        ]
        .spacing(6);
        widgets::list_row(
            t,
            line,
            self.cursor.as_ref() == Some(&fold),
            Message::ToggleFolder(section, path.to_owned()),
        )
    }

    fn branch_row<'a>(
        &self,
        t: &'a Tokens,
        b: &'a Branch,
        depth: usize,
        switching: Option<&'a str>,
    ) -> Element<'a, Message> {
        let full = b.name.as_str();
        // Inside folders only the last part of the name is shown.
        let short = b.name.short();
        let leaf = if depth > 0 {
            short.rsplit('/').next().unwrap_or(short)
        } else {
            short
        };
        let (marker, marker_color) = if b.is_head {
            ("●", t.head())
        } else {
            ("○", t.dim)
        };
        let name_color = if b.name.kind() == RefKind::RemoteBranch {
            t.remote()
        } else {
            t.branch()
        };
        let mut line = row![
            indent(t, depth),
            colored(t, marker, marker_color),
            colored(t, leaf, name_color).wrapping(Wrapping::None)
        ]
        .spacing(6);
        if let Some(frame) = switching {
            line = line.push(
                colored(t, format!("{frame} Switching…"), t.warning()).wrapping(Wrapping::None),
            );
        }
        if let Some(up) = &b.upstream {
            if up.gone {
                line = line.push(colored(t, "⚠ gone", t.warning()));
            } else {
                if up.ahead > 0 {
                    line = line.push(colored(t, format!("↑{}", up.ahead), t.success()));
                }
                if up.behind > 0 {
                    line = line.push(colored(t, format!("↓{}", up.behind), t.warning()));
                }
            }
        }
        let row = widgets::list_row(
            t,
            line,
            self.is_selected_ref(full),
            Message::Select(Selection::Ref(full.to_owned())),
        );
        let hint = match &b.upstream {
            Some(up) if up.gone => format!(
                "{} — upstream {} was deleted on the remote",
                b.name.short(),
                up.name.short()
            ),
            Some(up) => format!(
                "{} → {}  (↑{} ahead, ↓{} behind)",
                b.name.short(),
                up.name.short(),
                up.ahead,
                up.behind
            ),
            None if b.name.kind() == RefKind::LocalBranch => {
                format!("{} · not published yet", b.name.short())
            }
            None => format!("{} · {}", b.name.short(), b.target.short()),
        };
        widgets::tip(
            t,
            row,
            format!("{hint} · ⌘-click to mark · ⇧-click for a range"),
        )
    }

    fn tag_row<'a>(&self, t: &'a Tokens, tag: &'a Tag) -> Element<'a, Message> {
        let full = tag.name.as_str();
        let line = row![
            indent(t, 0),
            colored(t, "◆", t.tag()),
            colored(t, tag.name.short(), t.tag()).wrapping(Wrapping::None)
        ]
        .spacing(6);
        let row = widgets::list_row(
            t,
            line,
            self.is_selected_ref(full),
            Message::Select(Selection::Ref(full.to_owned())),
        );
        let hint = match &tag.message {
            Some(m) => format!(
                "{} → {} · {}",
                tag.name.short(),
                tag.target.short(),
                m.lines().next().unwrap_or("")
            ),
            None => format!(
                "{} → {} (lightweight)",
                tag.name.short(),
                tag.target.short()
            ),
        };
        widgets::tip(t, row, hint)
    }

    fn stash_row<'a>(&self, t: &'a Tokens, s: &'a StashEntry) -> Element<'a, Message> {
        let line = row![
            indent(t, 0),
            colored(t, "≡", t.accent()),
            colored(t, s.message.clone(), t.fg).wrapping(Wrapping::None)
        ]
        .spacing(6);
        let selected = self.selected == Some(Selection::Stash(s.index));
        widgets::tip(
            t,
            widgets::list_row(
                t,
                line,
                selected,
                Message::Select(Selection::Stash(s.index)),
            ),
            format!("{} · {}", s.refname(), format::datetime(s.time)),
        )
    }

    fn submodule_row<'a>(&self, t: &'a Tokens, m: &'a gf_git::Submodule) -> Element<'a, Message> {
        use gf_git::SubmoduleStatus as S;
        let (mark, color, hint) = match m.status {
            S::UpToDate => ("●", t.success(), "at the recorded commit"),
            S::Uninitialized => ("○", t.dim, "not checked out yet — update to fetch it"),
            S::Modified => (
                "✚",
                t.warning(),
                "checked out at a different commit than recorded",
            ),
            S::Conflict => ("✗", t.error(), "merge conflict"),
        };
        let line = row![
            indent(t, 0),
            colored(t, mark, color),
            colored(t, m.path.display().to_string(), t.fg).wrapping(Wrapping::None),
            dim(
                t,
                m.describe
                    .clone()
                    .unwrap_or_else(|| m.commit.short().to_owned())
            )
            .wrapping(Wrapping::None),
        ]
        .spacing(6);
        let sel = Selection::Submodule(m.path.clone());
        widgets::tip(
            t,
            widgets::list_row(
                t,
                line,
                self.selected.as_ref() == Some(&sel),
                Message::Select(sel),
            ),
            format!("{} · {hint}", m.path.display()),
        )
    }

    fn lfs_row<'a>(&self, t: &'a Tokens, pattern: &'a str) -> Element<'a, Message> {
        let line = row![
            indent(t, 0),
            colored(t, "▣", t.tag()),
            colored(t, pattern.to_owned(), t.fg).wrapping(Wrapping::None)
        ]
        .spacing(6);
        let sel = Selection::LfsPattern(pattern.to_owned());
        widgets::list_row(
            t,
            line,
            self.selected.as_ref() == Some(&sel),
            Message::Select(sel),
        )
    }

    /// Label of what is selected, for the actions button and menu title.
    fn selection_label(&self, snap: &Snapshot) -> Option<String> {
        Some(match self.selected.as_ref()? {
            Selection::Ref(r) => short_name(r).to_owned(),
            Selection::Stash(i) => snap.stashes.iter().find(|s| s.index == *i).map_or_else(
                || format!("stash@{{{i}}}"),
                |s| format::truncate(&s.message, 30),
            ),
            Selection::Submodule(p) => p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            Selection::LfsPattern(pat) => pat.clone(),
        })
    }

    /// What Enter (and a double click) does with `sel`: check a branch out,
    /// open a submodule. `None` for items with no default action.
    fn default_action(sel: &Selection, snap: &Snapshot) -> Option<Action> {
        match sel {
            Selection::Ref(r) => snap
                .branches
                .iter()
                .find(|b| b.name.as_str() == r)
                .filter(|b| !b.is_head)
                .map(|_| Action::Checkout),
            Selection::Submodule(_) => Some(Action::Open),
            Selection::Stash(_) | Selection::LfsPattern(_) => None,
        }
    }

    /// Actions for the current selection (a right-click, or `.`). `None`
    /// when there is nothing to do with it.
    fn menu(&self, snap: &Snapshot) -> Option<Menu<Message>> {
        if !self.marked_branches.is_empty() {
            let n = self.marked_branches.len();
            return Some(Menu {
                title: format!("{n} branches selected"),
                entries: vec![
                    Entry::caution(
                        'd',
                        "delete selected branches…",
                        "review the branches before deleting",
                        Message::Act(Action::Delete),
                    ),
                    Entry::item(
                        'c',
                        "clear selection",
                        "unmark the selected branches",
                        Message::ClearBranchMarks,
                    ),
                ],
            });
        }
        let item = |key: char, label: &str, hint: &str, a: Action| {
            Entry::item(key, label, hint, Message::Act(a))
        };
        let danger = |key: char, label: &str, hint: &str, a: Action| {
            Entry::caution(key, label, hint, Message::Act(a))
        };
        let held_elsewhere = match self.selected.as_ref() {
            Some(Selection::Ref(r)) => snap
                .branches
                .iter()
                .find(|b| b.name.as_str() == r)
                .is_some_and(|b| snap.worktree_for_checkout(b).is_some()),
            _ => false,
        };
        let checkout = || {
            let hint = if held_elsewhere {
                "open the worktree that has it"
            } else {
                "switch your files to it"
            };
            item('o', "checkout", hint, Action::Checkout)
        };
        // A branch another worktree has cannot go to a new one too.
        let new_worktree = || {
            (!held_elsewhere).then(|| {
                item(
                    'w',
                    "new worktree…",
                    "check it out in its own folder",
                    Action::NewWorktree,
                )
            })
        };
        let merge = || {
            item(
                'm',
                "merge into current",
                "bring its commits into the checked-out branch",
                Action::Merge,
            )
        };
        let rebase = || {
            item(
                'e',
                "rebase current onto",
                "replay the checked-out branch on top of it",
                Action::RebaseOnto,
            )
        };
        let compare = || {
            item(
                'c',
                "compare with current",
                "diff the checked-out branch against it",
                Action::Compare,
            )
        };
        let branch_here = || {
            item(
                'b',
                "branch from here…",
                "create a branch starting at it",
                Action::BranchHere,
            )
        };
        let entries = match self.selected.as_ref()? {
            Selection::Ref(r) => match snap.branches.iter().find(|b| b.name.as_str() == r) {
                Some(b) if b.name.kind() == RefKind::LocalBranch && b.is_head => vec![
                    branch_here(),
                    item('r', "rename…", "change the branch name", Action::Rename),
                    item('p', "push", "send its commits to the remote", Action::Push),
                ],
                Some(b) if b.name.kind() == RefKind::LocalBranch => [
                    Some(checkout()),
                    new_worktree(),
                    Some(merge()),
                    Some(rebase()),
                    Some(compare()),
                    Some(Entry::Separator),
                    Some(item(
                        'r',
                        "rename…",
                        "change the branch name",
                        Action::Rename,
                    )),
                    Some(item(
                        'p',
                        "push",
                        "send its commits to the remote",
                        Action::Push,
                    )),
                    Some(Entry::Separator),
                    Some(danger('d', "delete…", "remove the branch", Action::Delete)),
                ]
                .into_iter()
                .flatten()
                .collect(),
                Some(_) => [
                    Some(checkout()),
                    new_worktree(),
                    Some(merge()),
                    Some(rebase()),
                    Some(compare()),
                    Some(branch_here()),
                    Some(Entry::Separator),
                    Some(danger(
                        'd',
                        "delete…",
                        "remove it on the remote",
                        Action::Delete,
                    )),
                ]
                .into_iter()
                .flatten()
                .collect(),
                None => vec![
                    item('p', "push", "publish the tag to the remote", Action::Push),
                    Entry::Separator,
                    danger('d', "delete…", "remove the local tag", Action::Delete),
                ],
            },
            Selection::Stash(_) => vec![
                item(
                    'a',
                    "apply",
                    "put its changes back; keeps the stash",
                    Action::StashApply,
                ),
                item(
                    'p',
                    "pop",
                    "apply and remove it from the list",
                    Action::StashPop,
                ),
                Entry::Separator,
                danger('d', "drop…", "delete the stash", Action::Delete),
            ],
            Selection::Submodule(_) => vec![
                item('o', "open in tab", "open the submodule", Action::Open),
                item(
                    'u',
                    "update all",
                    "check out the recorded commits",
                    Action::UpdateSubmodules,
                ),
            ],
            Selection::LfsPattern(_) => vec![
                item(
                    'p',
                    "lfs pull",
                    "download the file contents",
                    Action::LfsPull,
                ),
                Entry::Separator,
                danger('d', "untrack…", "stop storing it in LFS", Action::Delete),
            ],
        };
        Some(Menu {
            title: self.selection_label(snap)?,
            entries,
        })
    }

    fn output_for(&self, action: Action, snap: Option<&Snapshot>) -> Option<Output> {
        match action {
            Action::NewBranch | Action::BranchHere => {
                let from = match &self.selected {
                    Some(Selection::Ref(r)) => Some(r.clone()),
                    _ => None,
                };
                return Some(Output::NewBranch {
                    from,
                    branch_here: action == Action::BranchHere,
                });
            }
            Action::NewTag => return Some(Output::NewTag),
            Action::NewStash => return Some(Output::NewStash),
            Action::UpdateSubmodules => return Some(Output::UpdateSubmodules),
            Action::TrackLfs => return Some(Output::TrackLfs),
            Action::LfsPull => return Some(Output::LfsPull),
            _ => {}
        }
        let snap = snap?;
        if action == Action::Delete && !self.marked_branches.is_empty() {
            let branches = snap
                .branches
                .iter()
                .filter(|b| self.marked_branches.contains(b.name.as_str()))
                .cloned()
                .collect();
            return Some(Output::DeleteBranches(branches));
        }
        match self.selected.as_ref()? {
            Selection::Submodule(p) => match action {
                Action::Open => Some(Output::OpenRepo(p.clone())),
                _ => None,
            },
            Selection::LfsPattern(pat) => match action {
                Action::Delete => Some(Output::UntrackLfs(pat.clone())),
                _ => None,
            },
            Selection::Stash(i) => match action {
                Action::StashApply => Some(Output::StashApply(*i)),
                Action::StashPop => Some(Output::StashPop(*i)),
                Action::Delete => Some(Output::StashDrop(*i)),
                _ => None,
            },
            Selection::Ref(r) => {
                if let Some(b) = snap.branches.iter().find(|b| b.name.as_str() == r) {
                    match action {
                        Action::Checkout => Some(Output::Checkout(b.clone())),
                        Action::Rename => Some(Output::RenameBranch(b.name.short().to_owned())),
                        Action::Delete => Some(Output::DeleteBranches(vec![b.clone()])),
                        Action::Push => Some(Output::PushBranch(b.clone())),
                        Action::Merge => Some(Output::Merge(b.name.short().to_owned())),
                        Action::RebaseOnto => Some(Output::RebaseOnto(b.name.short().to_owned())),
                        Action::Compare if !b.is_head => Some(Output::Compare(b.clone())),
                        Action::NewWorktree if !b.is_head => {
                            let short = b.name.short();
                            let local = match b.name.kind() {
                                RefKind::RemoteBranch => {
                                    short.split_once('/').map_or(short, |(_, name)| name)
                                }
                                _ => short,
                            };
                            Some(Output::NewWorktree(local.to_owned()))
                        }
                        _ => None,
                    }
                } else {
                    let tag = r.strip_prefix("refs/tags/")?.to_owned();
                    match action {
                        Action::Delete => Some(Output::DeleteTag(tag)),
                        Action::Push => Some(Output::PushTag(tag)),
                        _ => None,
                    }
                }
            }
        }
    }

    /// Like [`Feature::update`] but with access to the snapshot, which the
    /// action bar needs to resolve the selection into a branch/tag/stash.
    pub fn update_with(
        &mut self,
        message: Message,
        snap: Option<&Snapshot>,
    ) -> Update<Message, Output> {
        match message {
            Message::ToggleFolder(section, path) => {
                let fold = Fold::Folder(section, path);
                let folded = self.is_folded(&fold);
                self.set_folded(&fold, !folded);
                self.cursor = Some(fold);
                Update::none()
            }
            Message::Toggle(section) => {
                self.layout.take();
                self.cursor = Some(Fold::Section(section));
                if !self.collapsed.remove(&section) {
                    self.collapsed.insert(section);
                }
                Update::none()
            }
            Message::Filter(f) => {
                self.layout.take();
                self.filter = f;
                Update::none()
            }
            Message::ToggleBranch(ref name) | Message::SelectBranchRange(ref name) => {
                let range = matches!(message, Message::SelectBranchRange(_));
                if let Some(snap) = snap {
                    self.mark_branch(name.clone(), range, snap);
                }
                Update::none()
            }
            Message::ClearBranchMarks => {
                self.clear_branch_marks();
                Update::none()
            }
            Message::Select(sel) => {
                self.clear_branch_marks();
                // A double click does what Enter does (checkout, open …).
                let default = snap.and_then(|s| Self::default_action(&sel, s));
                let double = default.is_some()
                    && self
                        .last_click
                        .as_ref()
                        .is_some_and(|(s, at)| *s == sel && at.elapsed() < DOUBLE_CLICK);
                self.last_click = (!double).then(|| (sel.clone(), std::time::Instant::now()));
                if let (true, Some(action)) = (double, default) {
                    self.cursor = None;
                    self.selected = Some(sel);
                    return self
                        .output_for(action, snap)
                        .map_or_else(Update::none, Update::output);
                }
                let shows_history = matches!(sel, Selection::Ref(_) | Selection::Stash(_));
                // Coming back from a header (keyboard) re-picks the item
                // instead of unselecting it.
                let from_header = self.cursor.take().is_some();
                if self.selected.as_ref() == Some(&sel) && from_header {
                    return Update::none();
                }
                if self.selected.as_ref() == Some(&sel) {
                    self.selected = None;
                    return if shows_history {
                        Update::output(Output::ShowRevision(None))
                    } else {
                        Update::none()
                    };
                }
                self.selected = Some(sel.clone());
                match sel {
                    Selection::Ref(r) => Update::output(Output::ShowRevision(Some(r))),
                    Selection::Stash(i) => snap
                        .and_then(|s| s.stashes.get(i))
                        .map_or_else(Update::none, |s| {
                            Update::output(Output::ShowStash(s.clone()))
                        }),
                    // Selecting these only shows their actions.
                    Selection::Submodule(_) | Selection::LfsPattern(_) => Update::none(),
                }
            }
            Message::Collapse => Update::output(Output::Collapse),
            Message::OpenActions => match snap.and_then(|s| self.menu(s)) {
                Some(menu) => Update::output(Output::Menu { show: None, menu }),
                None => Update::none(),
            },
            Message::RightClick(sel) => {
                if matches!(&sel, Selection::Ref(r) if self.marked_branches.contains(r)) {
                    return snap
                        .and_then(|s| self.menu(s))
                        .map_or_else(Update::none, |menu| {
                            Update::output(Output::Menu { show: None, menu })
                        });
                }
                self.clear_branch_marks();
                // Selecting by right-click never unselects; the app shows
                // the item's history/diff like a left click would.
                let changed = self.selected.as_ref() != Some(&sel);
                self.cursor = None;
                self.selected = Some(sel.clone());
                match snap.and_then(|s| self.menu(s)) {
                    Some(menu) => Update::output(Output::Menu {
                        show: changed.then_some(sel),
                        menu,
                    }),
                    None => Update::none(),
                }
            }
            Message::Act(action) => {
                // Creating something (`+`, "branch from here…") opens the
                // section it will appear in, so the new item shows there.
                if let Some(section) = action.creates_in() {
                    self.layout.take();
                    self.collapsed.remove(&section);
                }
                self.output_for(action, snap)
                    .map_or_else(Update::none, Update::output)
            }
            Message::Scrolled(pos) => {
                self.scroll = pos;
                Update::none()
            }
        }
    }
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        self.update_with(message, None)
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        self.view_with_checkout(cx, None)
    }
}

impl State {
    pub fn view_with_checkout<'a>(
        &'a self,
        cx: Cx<'a>,
        checkout: Option<(&'a str, &'a str)>,
    ) -> Element<'a, Message> {
        let progress = move |branch: &Branch| {
            checkout
                .filter(|(target, _)| *target == branch.name.as_str())
                .map(|(_, frame)| frame)
        };
        let t = cx.tokens;
        let filter = text_input("filter refs…", &self.filter)
            .id(REFS_FILTER_ID)
            .on_input(Message::Filter)
            .size(t.small())
            .padding(3)
            .style(style::field_input(t));

        let Some(snap) = cx.snapshot else {
            return widgets::panel_with_action(
                t,
                "REFS".into(),
                widgets::icon_button(t, "«", t.accent(), "hide refs ⌘B", Message::Collapse),
                column![filter, widgets::empty_hint(t, "loading…")],
            );
        };

        let (rows, name_cols) = self.layout(snap);
        let indicator_cols = checkout.map_or(0, |_| columns("   Switching…"));
        let content_width = t.cols(4 + name_cols + indicator_cols) + 16.0;
        let list = virtual_list(
            LIST_ID,
            rows.len(),
            row_height(t),
            content_width,
            self.scroll,
            move |i| {
                let el: Element<'a, Message> = match &rows[i] {
                    SideRow::Header(sec, label, count, add) => {
                        self.header(t, *sec, label, *count, *add)
                    }
                    SideRow::Folder {
                        section,
                        path,
                        depth,
                        count,
                    } => self.folder_row(t, *section, path, *depth, *count),
                    SideRow::Branch(b, depth) => self.branch_row(t, b, *depth, progress(b)),
                    SideRow::Tag(tg) => self.tag_row(t, tg),
                    SideRow::Stash(s) => self.stash_row(t, s),
                    SideRow::Submodule(m) => self.submodule_row(t, m),
                    SideRow::Lfs(p) => self.lfs_row(t, p),
                    SideRow::Hint(h) => container(row![indent(t, 0), dim(t, *h)])
                        .padding(Padding::from([4, 6]))
                        .into(),
                    SideRow::UnbornBranch(name) => container(row![
                        indent(t, 0),
                        colored(t, format!("● {name} (no commits yet)"), t.head())
                    ])
                    .padding(Padding::from([1, 6]))
                    .into(),
                };
                match Item::of(&rows[i]) {
                    Some(Item::Pick(sel, _)) => iced::widget::mouse_area(el)
                        .on_right_press(Message::RightClick(sel))
                        .into(),
                    _ => el,
                }
            },
            Message::Scrolled,
        );

        let mut actions = row![].spacing(4);
        if !self.marked_branches.is_empty() {
            let label = format!("Delete {}…", self.marked_branches.len());
            actions = actions.push(widgets::danger_button(
                t,
                &label,
                Some(Message::Act(Action::Delete)),
            ));
        }
        actions = actions.push(widgets::icon_button(
            t,
            "«",
            t.accent(),
            "hide refs ⌘B",
            Message::Collapse,
        ));
        widgets::panel_with_action(
            t,
            "REFS".into(),
            actions,
            column![filter, list].spacing(4).width(Length::Fill),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gf_core::model::RefName;

    fn branch(name: &str, head: bool) -> Branch {
        Branch {
            name: RefName::local_branch(name),
            target: "a".repeat(40).parse().expect("oid"),
            upstream: None,
            is_head: head,
        }
    }

    fn snap() -> Snapshot {
        Snapshot {
            branches: [branch("main", true), branch("dev", false)].into(),
            ..Snapshot::default()
        }
    }

    #[test]
    fn select_toggles_and_reports() {
        let mut s = State::default();
        let out = s
            .update(Message::Select(Selection::Ref("refs/heads/dev".into())))
            .output;
        assert!(matches!(out, Some(Output::ShowRevision(Some(ref r))) if r == "refs/heads/dev"));
        let out = s
            .update(Message::Select(Selection::Ref("refs/heads/dev".into())))
            .output;
        assert!(matches!(out, Some(Output::ShowRevision(None))));
    }

    #[test]
    fn actions_resolve_against_snapshot() {
        let snap = snap();
        let mut s = State::default();
        let _ = s.update_with(
            Message::Select(Selection::Ref("refs/heads/dev".into())),
            Some(&snap),
        );
        let out = s
            .update_with(Message::Act(Action::Delete), Some(&snap))
            .output;
        assert!(
            matches!(out, Some(Output::DeleteBranches(bs)) if bs.len() == 1 && bs[0].name.short() == "dev")
        );
        let out = s
            .update_with(Message::Act(Action::NewBranch), Some(&snap))
            .output;
        assert!(
            matches!(out, Some(Output::NewBranch { from: Some(f), branch_here: false }) if f == "refs/heads/dev")
        );
    }

    #[test]
    fn checkout_hint_points_at_the_worktree_tab() {
        let mut snap = snap();
        snap.worktrees = vec![gf_git::Worktree {
            path: "/wt/dev".into(),
            head: None,
            branch: Some("dev".into()),
            bare: false,
            locked: false,
            prunable: false,
        }];
        let mut s = State::default();
        let _ = s.update_with(
            Message::Select(Selection::Ref("refs/heads/dev".into())),
            Some(&snap),
        );
        let hint = |s: &State, snap: &Snapshot| {
            let menu = s.menu(snap).expect("menu");
            menu.entries.iter().find_map(|e| match e {
                Entry::Item { key: 'o', hint, .. } => Some(hint.clone()),
                _ => None,
            })
        };
        assert_eq!(
            hint(&s, &snap).as_deref(),
            Some("open the worktree that has it")
        );
        snap.worktrees.clear();
        assert_eq!(hint(&s, &snap).as_deref(), Some("switch your files to it"));
    }

    #[test]
    fn a_branch_no_worktree_has_can_go_to_a_new_one() {
        let mut snap = snap();
        let mut s = State::default();
        let _ = s.update_with(
            Message::Select(Selection::Ref("refs/heads/dev".into())),
            Some(&snap),
        );
        let offers = |s: &State, snap: &Snapshot| {
            s.menu(snap)
                .expect("menu")
                .entries
                .iter()
                .any(|e| matches!(e, Entry::Item { key: 'w', .. }))
        };
        assert!(offers(&s, &snap));
        let out = s
            .update_with(Message::Act(Action::NewWorktree), Some(&snap))
            .output;
        assert!(matches!(out, Some(Output::NewWorktree(b)) if b == "dev"));
        // Held by another worktree: that one is opened instead (checkout).
        snap.worktrees = vec![gf_git::Worktree {
            path: "/wt/dev".into(),
            head: None,
            branch: Some("dev".into()),
            bare: false,
            locked: false,
            prunable: false,
        }];
        assert!(!offers(&s, &snap));
        // The checked-out branch is already in this worktree.
        let _ = s.update_with(
            Message::Select(Selection::Ref("refs/heads/main".into())),
            Some(&snap),
        );
        assert!(!offers(&s, &snap));
    }

    #[test]
    fn filter_is_case_insensitive() {
        let mut s = State::default();
        let _ = s.update(Message::Filter("FEAT".into()));
        assert!(s.matches("feature/login"));
        assert!(!s.matches("main"));
    }

    #[test]
    fn removed_selection_is_forgotten() {
        let mut s = State::default();
        let _ = s.update(Message::Select(Selection::Stash(3)));
        assert!(!s.retain_existing(&snap()));
        assert!(s.selected.is_none());
    }

    #[test]
    fn arrows_walk_headers_and_items_and_fold_sections() {
        let t = crate::theme::preset("gf-dark");
        let snap = snap();
        let mut s = State::default();
        let key = |s: &mut State, k| s.nav(Nav::plain(k), Some(&snap), &t).map(|m| m.message);
        // ↓ from nothing: the BRANCHES header.
        assert!(matches!(key(&mut s, Key::Down), Some(None)));
        assert_eq!(s.cursor, Some(Fold::Section(Section::Branches)));
        // Next ↓ picks the first branch.
        let Some(Some(Message::Select(sel))) = key(&mut s, Key::Down) else {
            panic!("selects the branch");
        };
        let _ = s.update_with(Message::Select(sel), Some(&snap));
        assert!(s.cursor.is_none() && s.selected.is_some());
        // ← folds the section and moves the cursor to its header.
        assert!(matches!(key(&mut s, Key::Left), Some(None)));
        assert!(s.collapsed.contains(&Section::Branches));
        assert_eq!(s.cursor, Some(Fold::Section(Section::Branches)));
        // → unfolds it again; a second → is left to the app.
        assert!(matches!(key(&mut s, Key::Right), Some(None)));
        assert!(!s.collapsed.contains(&Section::Branches));
        assert!(key(&mut s, Key::Right).is_none());
        // Enter on a header toggles it.
        assert!(matches!(
            key(&mut s, Key::Enter),
            Some(Some(Message::Toggle(Section::Branches)))
        ));
    }

    #[test]
    fn menu_lists_actions_per_selection_and_right_click_selects() {
        let snap = snap();
        let mut s = State::default();
        // Nothing selected: no menu.
        assert!(
            s.update_with(Message::OpenActions, Some(&snap))
                .output
                .is_none()
        );
        // Right-click on another branch selects it and offers checkout/merge/delete.
        let out = s
            .update_with(
                Message::RightClick(Selection::Ref("refs/heads/dev".into())),
                Some(&snap),
            )
            .output;
        let Some(Output::Menu { show, menu }) = out else {
            panic!("menu expected");
        };
        assert!(matches!(show, Some(Selection::Ref(ref r)) if r == "refs/heads/dev"));
        assert_eq!(menu.title, "dev");
        let keys: Vec<char> = menu
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item { key, .. } => Some(*key),
                Entry::Separator => None,
            })
            .collect();
        assert_eq!(keys, vec!['o', 'w', 'm', 'e', 'c', 'r', 'p', 'd']);
        // The checked-out branch cannot be merged into itself.
        let _ = s.update_with(
            Message::RightClick(Selection::Ref("refs/heads/main".into())),
            Some(&snap),
        );
        let Some(Output::Menu { show, menu }) =
            s.update_with(Message::OpenActions, Some(&snap)).output
        else {
            panic!("menu expected");
        };
        assert!(show.is_none(), "`.` does not change the selection");
        assert!(
            !menu
                .entries
                .iter()
                .any(|e| matches!(e, Entry::Item { key: 'm', .. }))
        );
        assert!(
            !menu
                .entries
                .iter()
                .any(|e| matches!(e, Entry::Item { key: 'c', .. })),
            "the checked-out branch cannot be compared with itself"
        );
    }

    #[test]
    fn double_click_checks_a_branch_out_like_enter() {
        let snap = snap();
        let dev = || Message::Select(Selection::Ref("refs/heads/dev".into()));
        let mut s = State::default();
        // First click selects and shows history.
        let out = s.update_with(dev(), Some(&snap)).output;
        assert!(matches!(out, Some(Output::ShowRevision(Some(_)))));
        // Second click right after: checkout, selection kept.
        let out = s.update_with(dev(), Some(&snap)).output;
        assert!(matches!(out, Some(Output::Checkout(ref b)) if b.name.short() == "dev"));
        assert!(s.selected.is_some());
        // The checked-out branch has no default action: two clicks still toggle.
        let main = || Message::Select(Selection::Ref("refs/heads/main".into()));
        let _ = s.update_with(main(), Some(&snap));
        let out = s.update_with(main(), Some(&snap)).output;
        assert!(matches!(out, Some(Output::ShowRevision(None))));
    }

    #[test]
    fn only_branches_start_open() {
        let s = State::default();
        assert!(!s.collapsed.contains(&Section::Branches));
        for sec in [
            Section::Remotes,
            Section::Tags,
            Section::Stashes,
            Section::Submodules,
            Section::Lfs,
        ] {
            assert!(s.collapsed.contains(&sec), "{sec:?} starts closed");
        }
    }

    #[test]
    fn creating_opens_the_section_the_new_item_goes_to() {
        let snap = snap();
        for (action, sec) in [
            (Action::NewTag, Section::Tags),
            (Action::NewStash, Section::Stashes),
            (Action::TrackLfs, Section::Lfs),
        ] {
            let mut s = State::default();
            assert!(s.collapsed.contains(&sec));
            let out = s.update_with(Message::Act(action), Some(&snap)).output;
            assert!(out.is_some(), "{action:?} still asks the app");
            assert!(!s.collapsed.contains(&sec), "{sec:?} opens");
        }
        let mut s = State::default();
        let _ = s.update_with(Message::Toggle(Section::Branches), Some(&snap));
        let _ = s.update_with(Message::Act(Action::NewBranch), Some(&snap));
        assert!(!s.collapsed.contains(&Section::Branches));
    }

    fn remote(name: &str) -> Branch {
        Branch {
            name: RefName::new(format!("refs/remotes/{name}")),
            target: "a".repeat(40).parse().expect("oid"),
            upstream: None,
            is_head: false,
        }
    }

    /// `(depth, label)` per row: `folder/` or a branch's short name.
    fn shape(rows: &[SideRow]) -> Vec<(usize, String)> {
        rows.iter()
            .map(|r| match r {
                SideRow::Folder { path, depth, .. } => (*depth, format!("{path}/")),
                SideRow::Branch(b, d) => (*d, b.name.short().to_owned()),
                _ => (99, String::new()),
            })
            .collect()
    }

    #[test]
    fn branches_nest_under_slash_folders() {
        let bs = [
            branch("main", true),
            branch("feature/dasf", false),
            branch("feature/abc", false),
            branch("feature/ui/x", false),
        ];
        let refs: Vec<&Branch> = bs.iter().collect();
        let rows = branch_tree(Section::Branches, &refs, |_| false, false);
        assert_eq!(
            shape(&rows),
            vec![
                (0, "feature/".into()),
                (1, "feature/abc".into()),
                (1, "feature/dasf".into()),
                (1, "feature/ui/".into()),
                (2, "feature/ui/x".into()),
                (0, "main".into()),
            ]
        );
        assert!(matches!(rows[0], SideRow::Folder { count: 3, .. }));
        // A closed folder keeps its row and hides what is inside.
        let rows = branch_tree(Section::Branches, &refs, |p| p == "feature", false);
        assert_eq!(
            shape(&rows),
            vec![(0, "feature/".into()), (0, "main".into())]
        );
        // While filtering, closed folders still show their matches.
        let rows = branch_tree(Section::Branches, &refs, |p| p == "feature", true);
        assert_eq!(rows.len(), 6);
    }

    #[test]
    fn remote_branches_nest_under_their_remote() {
        let bs = [remote("origin/main"), remote("origin/feature/a")];
        let refs: Vec<&Branch> = bs.iter().collect();
        let rows = branch_tree(Section::Remotes, &refs, |_| false, false);
        assert_eq!(
            shape(&rows),
            vec![
                (0, "origin/".into()),
                (1, "origin/feature/".into()),
                (2, "origin/feature/a".into()),
                (1, "origin/main".into()),
            ]
        );
    }

    #[test]
    fn keys_walk_and_fold_branch_folders() {
        let t = crate::theme::preset("gf-dark");
        let snap = Snapshot {
            branches: [
                branch("main", true),
                branch("feature/abc", false),
                branch("feature/dasf", false),
            ]
            .into(),
            ..Snapshot::default()
        };
        let mut s = State::default();
        let key = |s: &mut State, k| s.nav(Nav::plain(k), Some(&snap), &t).map(|m| m.message);
        let _ = key(&mut s, Key::Down); // BRANCHES header
        let _ = key(&mut s, Key::Down); // feature/ folder
        let folder = Fold::Folder(Section::Branches, "feature".into());
        assert_eq!(s.cursor, Some(folder.clone()));
        // ↓ onto a branch inside, then ← closes the folder around it.
        let Some(Some(Message::Select(sel))) = key(&mut s, Key::Down) else {
            panic!("selects feature/abc");
        };
        let _ = s.update_with(Message::Select(sel), Some(&snap));
        let _ = key(&mut s, Key::Left);
        assert_eq!(s.cursor, Some(folder.clone()));
        assert!(s.is_folded(&folder));
        // → opens it again; Enter toggles it.
        let _ = key(&mut s, Key::Right);
        assert!(!s.is_folded(&folder));
        let Some(Some(m)) = key(&mut s, Key::Enter) else {
            panic!("toggle");
        };
        let _ = s.update_with(m, Some(&snap));
        assert!(s.is_folded(&folder));
    }
}
