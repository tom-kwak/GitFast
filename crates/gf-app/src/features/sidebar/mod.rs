//! Sidebar: local branches, remote branches, tags and stashes as a
//! collapsible tree with a quick filter. Selecting an item shows its
//! history (or diff, for stashes) and an action bar for it; section headers
//! have `+` buttons to create new items.

use std::collections::HashSet;

use gf_core::model::{Branch, RefKind, StashEntry, Tag};
use iced::widget::text::Wrapping;
use iced::widget::{button, column, row, space, text_input};
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
    Worktrees,
    Submodules,
    Lfs,
}

/// What is selected in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Full ref name of a branch or tag.
    Ref(String),
    Stash(usize),
    Worktree(std::path::PathBuf),
    Submodule(std::path::PathBuf),
    LfsPattern(String),
}

#[derive(Debug, Default)]
pub struct State {
    collapsed: HashSet<Section>,
    filter: String,
    selected: Option<Selection>,
    /// The keyboard cursor is on this section header (instead of the
    /// selected item).
    header_cursor: Option<Section>,
    /// The last item clicked and when, to recognize a double click (iced
    /// cannot: the row's button consumes the press).
    last_click: Option<(Selection, std::time::Instant)>,
    scroll: ScrollPos,
}

/// Two clicks on the same item within this time are a double click.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

/// Rows the keyboard can stop on.
#[derive(Debug, Clone)]
enum Item {
    Header(Section),
    /// An item and what Enter does with it.
    Pick(Selection, Option<Action>),
}

impl Item {
    fn of(row: &SideRow<'_>) -> Option<Self> {
        Some(match row {
            SideRow::Header(sec, ..) => Self::Header(*sec),
            SideRow::Branch(b) => Self::Pick(
                Selection::Ref(b.name.as_str().to_owned()),
                (!b.is_head).then_some(Action::Checkout),
            ),
            SideRow::Tag(tg) => Self::Pick(Selection::Ref(tg.name.as_str().to_owned()), None),
            SideRow::Stash(st) => Self::Pick(Selection::Stash(st.index), None),
            SideRow::Worktree(w) => {
                Self::Pick(Selection::Worktree(w.path.clone()), Some(Action::Open))
            }
            SideRow::Submodule(m) => {
                Self::Pick(Selection::Submodule(m.path.clone()), Some(Action::Open))
            }
            SideRow::Lfs(p) => Self::Pick(Selection::LfsPattern((*p).clone()), None),
            SideRow::Hint(_) => return None,
        })
    }
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

/// One row of the flattened tree.
#[derive(Debug, Clone, Copy)]
enum SideRow<'a> {
    Header(Section, &'static str, usize, Option<(Action, &'static str)>),
    Branch(&'a Branch),
    Tag(&'a Tag),
    Stash(&'a StashEntry),
    Worktree(&'a gf_git::Worktree),
    Submodule(&'a gf_git::Submodule),
    Lfs(&'a String),
    Hint(&'static str),
}

#[derive(Debug, Clone)]
pub enum Message {
    Toggle(Section),
    Filter(String),
    Select(Selection),
    /// Header `+` buttons and the actions menu entries.
    Act(Action),
    /// The `actions ▾` button (or `.`): menu for the selection.
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
    NewWorktree,
    /// Open the selected worktree / submodule in a new tab.
    Open,
    UpdateSubmodules,
    TrackLfs,
    LfsPull,
}

/// Requests to the app. Destructive ones are confirmed by the app.
#[derive(Debug, Clone)]
pub enum Output {
    /// Show history of this ref; `None` = back to the default history.
    ShowRevision(Option<String>),
    ShowStash(StashEntry),
    NewBranch {
        from: Option<String>,
    },
    NewTag,
    NewStash,
    Checkout(Branch),
    RenameBranch(String),
    DeleteBranch(Branch),
    PushBranch(Branch),
    /// Merge this branch (short name) into the current branch.
    Merge(String),
    /// Rebase the current branch onto this branch (short name).
    RebaseOnto(String),
    NewWorktree,
    RemoveWorktree(std::path::PathBuf),
    /// Open a repository folder (worktree / submodule) in a new tab.
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
        let exists = match &self.selected {
            None => return true,
            Some(Selection::Ref(r)) => {
                snapshot.branches.iter().any(|b| b.name.as_str() == r)
                    || snapshot.tags.iter().any(|t| t.name.as_str() == r)
            }
            Some(Selection::Stash(i)) => *i < snapshot.stashes.len(),
            Some(Selection::Worktree(p)) => snapshot.worktrees.iter().any(|w| &w.path == p),
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
        self.selected = None;
    }

    fn matches(&self, name: &str) -> bool {
        self.filter.is_empty() || name.to_lowercase().contains(&self.filter.to_lowercase())
    }

    fn is_selected_ref(&self, full: &str) -> bool {
        matches!(&self.selected, Some(Selection::Ref(r)) if r == full)
    }

    /// The flattened tree as shown.
    fn rows<'a>(&self, snap: &'a Snapshot) -> Vec<SideRow<'a>> {
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
        let mut rows: Vec<SideRow<'a>> = Vec::new();
        let open = |sec: Section| !self.collapsed.contains(&sec);

        rows.push(SideRow::Header(
            Section::Branches,
            "BRANCHES",
            locals.len(),
            Some((Action::NewBranch, "new branch…")),
        ));
        if open(Section::Branches) {
            if locals.is_empty() {
                rows.push(SideRow::Hint(if self.filter.is_empty() {
                    "none yet — make a first commit"
                } else {
                    "no match"
                }));
            }
            rows.extend(locals.iter().map(|b| SideRow::Branch(b)));
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
            rows.extend(remotes.iter().map(|b| SideRow::Branch(b)));
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
            rows.extend(tags.iter().map(|tg| SideRow::Tag(tg)));
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
            rows.extend(stashes.iter().map(|s| SideRow::Stash(s)));
        }

        // Optional sections: only shown when the repository has them.
        if snap.worktrees.len() > 1 || !self.filter.is_empty() && !snap.worktrees.is_empty() {
            let wts: Vec<_> = snap
                .worktrees
                .iter()
                .filter(|w| self.matches(&w.path.display().to_string()))
                .collect();
            rows.push(SideRow::Header(
                Section::Worktrees,
                "WORKTREES",
                wts.len(),
                Some((Action::NewWorktree, "add a working tree…")),
            ));
            if open(Section::Worktrees) {
                rows.extend(wts.into_iter().map(SideRow::Worktree));
            }
        } else {
            rows.push(SideRow::Header(
                Section::Worktrees,
                "WORKTREES",
                0,
                Some((Action::NewWorktree, "add a working tree…")),
            ));
            if open(Section::Worktrees) {
                rows.push(SideRow::Hint(
                    "none — + checks a branch out in another folder",
                ));
            }
        }
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
                rows.extend(subs.into_iter().map(SideRow::Submodule));
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
                rows.extend(pats.into_iter().map(SideRow::Lfs));
            }
        }
        rows
    }

    /// Keys for the tree: arrows move over headers and items (an item is
    /// selected as the cursor reaches it), ←/→ fold and unfold sections,
    /// Enter/Space fold a header or check out / open the item.
    pub fn nav(&mut self, nav: Nav, snap: Option<&Snapshot>, t: &Tokens) -> Option<Moved<Message>> {
        let snap = snap?;
        let items: Vec<(usize, Item)> = self
            .rows(snap)
            .iter()
            .enumerate()
            .filter_map(|(i, r)| Item::of(r).map(|it| (i, it)))
            .collect();
        let current = items.iter().position(|(_, it)| match it {
            Item::Header(sec) => self.header_cursor == Some(*sec),
            Item::Pick(sel, _) => {
                self.header_cursor.is_none() && self.selected.as_ref() == Some(sel)
            }
        });
        let here = current.map(|c| items[c].1.clone());
        // Section of the cursor: the header itself or the nearest one above.
        let section = current.and_then(|c| {
            items[..=c].iter().rev().find_map(|(_, it)| match it {
                Item::Header(sec) => Some(*sec),
                Item::Pick(..) => None,
            })
        });
        match nav.key {
            Key::Left => {
                let sec = section?;
                self.collapsed.insert(sec);
                self.header_cursor = Some(sec);
                return Some(Moved::stay());
            }
            Key::Right => {
                return match here {
                    Some(Item::Header(sec)) if self.collapsed.contains(&sec) => {
                        self.collapsed.remove(&sec);
                        Some(Moved::stay())
                    }
                    _ => None,
                };
            }
            Key::Char('.') => {
                return self
                    .selected
                    .as_ref()
                    .map(|_| Moved::message(Message::OpenActions));
            }
            Key::Enter | Key::Space => {
                return match here? {
                    Item::Header(sec) => Some(Moved::message(Message::Toggle(sec))),
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
        let (row, item) = items[i].clone();
        let reveal = nav::reveal(LIST_ID, self.scroll, row, h);
        Some(match item {
            Item::Header(sec) => {
                self.header_cursor = Some(sec);
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
        let toggle = button(row![dim(t, arrow), dim(t, format!("{label} ({count})"))].spacing(6))
            .padding(Padding::from([1, 6]))
            .style(style::row_button(t, self.header_cursor == Some(section)))
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

    fn branch_row<'a>(&self, t: &'a Tokens, b: &'a Branch) -> Element<'a, Message> {
        let full = b.name.as_str();
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
            colored(t, marker, marker_color),
            colored(t, b.name.short(), name_color).wrapping(Wrapping::None)
        ]
        .spacing(6);
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
        widgets::tip(t, row, hint)
    }

    fn tag_row<'a>(&self, t: &'a Tokens, tag: &'a Tag) -> Element<'a, Message> {
        let full = tag.name.as_str();
        let line = row![
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

    fn worktree_row<'a>(&self, t: &'a Tokens, w: &'a gf_git::Worktree) -> Element<'a, Message> {
        let name = w.path.file_name().map_or_else(
            || w.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut line = row![
            colored(t, "⌂", t.accent()),
            colored(t, name, t.fg).wrapping(Wrapping::None)
        ]
        .spacing(6);
        line = line.push(
            dim(t, w.branch.clone().unwrap_or_else(|| "detached".into())).wrapping(Wrapping::None),
        );
        if w.locked {
            line = line.push(colored(t, "locked", t.warning()));
        }
        if w.prunable {
            line = line.push(colored(t, "missing", t.error()));
        }
        let sel = Selection::Worktree(w.path.clone());
        widgets::tip(
            t,
            widgets::list_row(
                t,
                line,
                self.selected.as_ref() == Some(&sel),
                Message::Select(sel),
            ),
            format::tilde(&w.path),
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
            Selection::Worktree(p) | Selection::Submodule(p) => p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            Selection::LfsPattern(pat) => pat.clone(),
        })
    }

    /// What Enter (and a double click) does with `sel`: check a branch out,
    /// open a worktree or submodule. `None` for items with no default action.
    fn default_action(sel: &Selection, snap: &Snapshot) -> Option<Action> {
        match sel {
            Selection::Ref(r) => snap
                .branches
                .iter()
                .find(|b| b.name.as_str() == r)
                .filter(|b| !b.is_head)
                .map(|_| Action::Checkout),
            Selection::Worktree(_) | Selection::Submodule(_) => Some(Action::Open),
            Selection::Stash(_) | Selection::LfsPattern(_) => None,
        }
    }

    /// Actions for the current selection (the `actions ▾` button, a
    /// right-click, or `.`). `None` when there is nothing to do with it.
    fn menu(&self, snap: &Snapshot) -> Option<Menu<Message>> {
        let item = |key: char, label: &str, hint: &str, a: Action| {
            Entry::item(key, label, hint, Message::Act(a))
        };
        let danger = |key: char, label: &str, hint: &str, a: Action| {
            Entry::caution(key, label, hint, Message::Act(a))
        };
        let checkout = || item('o', "checkout", "switch your files to it", Action::Checkout);
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
        let branch_here = || {
            item(
                'b',
                "branch from here…",
                "create a branch starting at it",
                Action::NewBranch,
            )
        };
        let entries = match self.selected.as_ref()? {
            Selection::Ref(r) => match snap.branches.iter().find(|b| b.name.as_str() == r) {
                Some(b) if b.name.kind() == RefKind::LocalBranch && b.is_head => vec![
                    branch_here(),
                    item('r', "rename…", "change the branch name", Action::Rename),
                    item('p', "push", "send its commits to the remote", Action::Push),
                ],
                Some(b) if b.name.kind() == RefKind::LocalBranch => vec![
                    checkout(),
                    merge(),
                    rebase(),
                    Entry::Separator,
                    item('r', "rename…", "change the branch name", Action::Rename),
                    item('p', "push", "send its commits to the remote", Action::Push),
                    Entry::Separator,
                    danger('d', "delete…", "remove the branch", Action::Delete),
                ],
                Some(_) => vec![
                    checkout(),
                    merge(),
                    rebase(),
                    branch_here(),
                    Entry::Separator,
                    danger('d', "delete…", "remove it on the remote", Action::Delete),
                ],
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
            Selection::Worktree(p) => {
                // The main working tree is the repository itself.
                if snap.worktrees.first().is_some_and(|w| &w.path == p) {
                    return None;
                }
                vec![
                    item('o', "open in tab", "open this working tree", Action::Open),
                    Entry::Separator,
                    danger(
                        'd',
                        "remove…",
                        "delete the working tree folder",
                        Action::Delete,
                    ),
                ]
            }
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

    /// Footer: the `actions ▾` button for the selection, or a hint.
    fn actions<'a>(&self, t: &'a Tokens, snap: &Snapshot) -> Option<Element<'a, Message>> {
        let label = self.selection_label(snap)?;
        let button: Element<'a, Message> = if self.menu(snap).is_some() {
            widgets::tip(
                t,
                widgets::text_button(t, "actions ▾", None, Some(Message::OpenActions)),
                "all actions for the selection · or right-click an item · or `.`".into(),
            )
        } else {
            dim(t, "the main working tree").into()
        };
        Some(
            row![button, colored(t, label, t.fg).wrapping(Wrapping::None)]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .into(),
        )
    }

    fn output_for(&self, action: Action, snap: Option<&Snapshot>) -> Option<Output> {
        match action {
            Action::NewBranch => {
                let from = match &self.selected {
                    Some(Selection::Ref(r)) => Some(r.clone()),
                    _ => None,
                };
                return Some(Output::NewBranch { from });
            }
            Action::NewTag => return Some(Output::NewTag),
            Action::NewStash => return Some(Output::NewStash),
            Action::NewWorktree => return Some(Output::NewWorktree),
            Action::UpdateSubmodules => return Some(Output::UpdateSubmodules),
            Action::TrackLfs => return Some(Output::TrackLfs),
            Action::LfsPull => return Some(Output::LfsPull),
            _ => {}
        }
        let snap = snap?;
        match self.selected.as_ref()? {
            Selection::Worktree(p) => match action {
                Action::Open => Some(Output::OpenRepo(p.clone())),
                Action::Delete => Some(Output::RemoveWorktree(p.clone())),
                _ => None,
            },
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
                        Action::Delete => Some(Output::DeleteBranch(b.clone())),
                        Action::Push => Some(Output::PushBranch(b.clone())),
                        Action::Merge => Some(Output::Merge(b.name.short().to_owned())),
                        Action::RebaseOnto => Some(Output::RebaseOnto(b.name.short().to_owned())),
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
            Message::Toggle(section) => {
                self.header_cursor = Some(section);
                if !self.collapsed.remove(&section) {
                    self.collapsed.insert(section);
                }
                Update::none()
            }
            Message::Filter(f) => {
                self.filter = f;
                Update::none()
            }
            Message::Select(sel) => {
                // A double click does what Enter does (checkout, open …).
                let default = snap.and_then(|s| Self::default_action(&sel, s));
                let double = default.is_some()
                    && self
                        .last_click
                        .as_ref()
                        .is_some_and(|(s, at)| *s == sel && at.elapsed() < DOUBLE_CLICK);
                self.last_click = (!double).then(|| (sel.clone(), std::time::Instant::now()));
                if let (true, Some(action)) = (double, default) {
                    self.header_cursor = None;
                    self.selected = Some(sel);
                    return self
                        .output_for(action, snap)
                        .map_or_else(Update::none, Update::output);
                }
                let shows_history = matches!(sel, Selection::Ref(_) | Selection::Stash(_));
                // Coming back from a header (keyboard) re-picks the item
                // instead of unselecting it.
                let from_header = self.header_cursor.take().is_some();
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
                    Selection::Worktree(_) | Selection::Submodule(_) | Selection::LfsPattern(_) => {
                        Update::none()
                    }
                }
            }
            Message::Collapse => Update::output(Output::Collapse),
            Message::OpenActions => match snap.and_then(|s| self.menu(s)) {
                Some(menu) => Update::output(Output::Menu { show: None, menu }),
                None => Update::none(),
            },
            Message::RightClick(sel) => {
                // Selecting by right-click never unselects; the app shows
                // the item's history/diff like a left click would.
                let changed = self.selected.as_ref() != Some(&sel);
                self.header_cursor = None;
                self.selected = Some(sel.clone());
                match snap.and_then(|s| self.menu(s)) {
                    Some(menu) => Update::output(Output::Menu {
                        show: changed.then_some(sel),
                        menu,
                    }),
                    None => Update::none(),
                }
            }
            Message::Act(action) => self
                .output_for(action, snap)
                .map_or_else(Update::none, Update::output),
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

        let rows = self.rows(snap);
        let name_cols = rows
            .iter()
            .map(|r| match r {
                SideRow::Branch(b) => columns(b.name.short()) + 12,
                SideRow::Tag(tg) => columns(tg.name.short()),
                SideRow::Stash(s) => columns(&s.message),
                SideRow::Worktree(w) => columns(&w.path.display().to_string()) + 20,
                SideRow::Submodule(m) => columns(&m.path.display().to_string()) + 24,
                SideRow::Lfs(p) => columns(p),
                SideRow::Hint(h) => columns(h),
                SideRow::Header(..) => 18,
            })
            .max()
            .unwrap_or(0);
        let content_width = t.cols(4 + name_cols) + 16.0;
        let list = virtual_list(
            LIST_ID,
            rows.len(),
            row_height(t),
            content_width,
            self.scroll,
            move |i| {
                let el: Element<'a, Message> = match rows[i] {
                    SideRow::Header(sec, label, count, add) => {
                        self.header(t, sec, label, count, add)
                    }
                    SideRow::Branch(b) => self.branch_row(t, b),
                    SideRow::Tag(tg) => self.tag_row(t, tg),
                    SideRow::Stash(s) => self.stash_row(t, s),
                    SideRow::Worktree(w) => self.worktree_row(t, w),
                    SideRow::Submodule(m) => self.submodule_row(t, m),
                    SideRow::Lfs(p) => self.lfs_row(t, p),
                    SideRow::Hint(h) => widgets::empty_hint(t, h),
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

        let footer: Element<'a, Message> = self.actions(t, snap).unwrap_or_else(|| {
            row![
                dim(t, "select an item (or right-click it) for actions"),
                space()
            ]
            .into()
        });
        widgets::panel_with_action(
            t,
            "REFS".into(),
            widgets::icon_button(t, "«", t.accent(), "hide refs ⌘B", Message::Collapse),
            column![filter, list, footer].spacing(4).width(Length::Fill),
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
            branches: vec![branch("main", true), branch("dev", false)],
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
        assert!(matches!(out, Some(Output::DeleteBranch(b)) if b.name.short() == "dev"));
        let out = s
            .update_with(Message::Act(Action::NewBranch), Some(&snap))
            .output;
        assert!(matches!(out, Some(Output::NewBranch { from: Some(f) }) if f == "refs/heads/dev"));
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
        assert_eq!(s.header_cursor, Some(Section::Branches));
        // Next ↓ picks the first branch.
        let Some(Some(Message::Select(sel))) = key(&mut s, Key::Down) else {
            panic!("selects the branch");
        };
        let _ = s.update_with(Message::Select(sel), Some(&snap));
        assert!(s.header_cursor.is_none() && s.selected.is_some());
        // ← folds the section and moves the cursor to its header.
        assert!(matches!(key(&mut s, Key::Left), Some(None)));
        assert!(s.collapsed.contains(&Section::Branches));
        assert_eq!(s.header_cursor, Some(Section::Branches));
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
        assert_eq!(keys, vec!['o', 'm', 'e', 'r', 'p', 'd']);
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
        assert!(show.is_none(), "button does not change the selection");
        assert!(
            !menu
                .entries
                .iter()
                .any(|e| matches!(e, Entry::Item { key: 'm', .. }))
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
}
