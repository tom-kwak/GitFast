//! Worktrees view: the second mode of the refs panel (`REFS │ WORKTREES`,
//! ⌘⇧B). One card per working tree of the repository, for watching and
//! steering several trees at once (e.g. one per AI agent):
//!
//! ```text
//!  ● main                               CLEAN
//!      clean
//!      Bump version 2h
//!
//! main ← 4 to merge · 2 new
//! └◉ feature/payment              EDITING 1m
//!      3 uncommitted · ⚠ 1 shared
//!      ↑2 · Add refund flow 15m
//! ```
//!
//! Each card hangs (`└─`) from a title naming the branch it merges into,
//! whether or not a worktree has that branch checked out (it may be only
//! a ref, or a remote branch). Most text is dim: color is kept for the
//! names and for what needs attention (the state word, conflicts, `⚠`).
//!
//! - Title: the base and how the tree stands against it: commits not in
//!   it yet (`to merge`), commits it got since (`new`), or `all merged`.
//!   Its tooltip says when nothing recorded where the branch came from (a
//!   guess `b` corrects). The tree on the base itself has no title and no
//!   `└─`: the others hang below it.
//! - Name line: branch, lock, and one word for what the tree is up to
//!   ([`Phase`]: `CONFLICT`, `EDITING` when files changed in the last 2
//!   minutes, `UNCOMMITTED`, `READY` to merge, `MERGED`, `BEHIND`, `CLEAN`,
//!   or `MISSING` / `ERROR`). After the `└`, `●` marks the tree this tab
//!   shows and `◉` one open in another tab. That tree is also the selected
//!   card until another is picked, and again whenever the tab is opened
//!   from another tree.
//! - Changes: conflicts and uncommitted entries, and `⚠ N shared` when
//!   other trees change the same files (merging both may conflict).
//! - Upstream: commits to push / pull as `↑2 ↓1` (nothing when in sync,
//!   `⊘` with no upstream; the tooltip says it in words), then the last
//!   commit and its age. Narrow panels cut the subject first.
//!
//! The folder is not shown: the branch tells trees apart. Hovering the name
//! shows it; `y` copies it and `r` reveals it. Every action is in the
//! right-click menu (`.` from the keyboard), so the panel has no buttons
//! beyond `+` (and `prune` / `clean N` when they apply).
//!
//! Every line has a fixed height and the text is cut to the panel width, so
//! statuses coming and going never move anything. The order is stable: the
//! main tree first, then by name. Statuses are read in the background (see
//! [`State::request`]) only while the view is shown. ⏎ / a double click
//! opens a tree in a tab, `c` compares it with the base, `m` merges it into
//! the base (in the tree that has the base checked out), `b` sets the base,
//! `e`/`t` open it in the IDE / a terminal, `d` removes it, `.` lists all.
//!
//! The base is per tree: the branch it came from (recorded by gf when it
//! makes the branch, else read from git, else guessed; see
//! `gf_git::WorktreeStatus::base`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gf_core::model::Oid;
use gf_git::{Repository, Worktree, WorktreeStatus};
use iced::widget::text::Wrapping;
use iced::widget::{Row, button, column, container, mouse_area, responsive, row, space};
use iced::{Alignment, Element, Length, Padding, Task};

use crate::feature::{Cx, Feature, Update};
use crate::features::context_menu::{Entry, Menu};
use crate::nav::{self, Key, Moved, Nav};
use crate::theme::{Tokens, style};
use crate::widgets::virtual_list::{SCROLLBAR, ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, format};

/// Scrollable id of the card list.
const LIST_ID: &str = "gf-worktrees";

/// Two clicks on the same card within this time open it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Files changed this recently mark a tree as being worked on (`⚡`).
const ACTIVE_FOR: Duration = Duration::from_secs(120);

/// Padding inside a card (vertical, horizontal) and the gap below it.
const PAD_Y: f32 = 3.0;
const PAD_X: f32 = 6.0;
const GAP: f32 = 4.0;
/// Space between a card's columns (mark, text, badges).
const SPACING: f32 = 4.0;

/// Heights of a card's lines: the name at the base size, the three detail
/// lines at the small size. Fixed, so badges never move the text.
fn line_heights(t: &Tokens) -> (f32, f32) {
    ((t.font_size * 1.35).ceil(), (t.small() * 1.35).ceil())
}

/// A card: four lines, its padding and the gap to the next one.
pub fn row_height(t: &Tokens) -> f32 {
    let (big, small) = line_heights(t);
    big + 3.0 * small + 2.0 * PAD_Y + 2.0 + GAP
}

/// Width of the column in front of the name: the `└─` tying the card to
/// its title, with `●` / `◉` for a tree open in a tab.
fn gutter(t: &Tokens) -> f32 {
    t.char_width() * 2.5
}

/// Small-size columns a card line has for text and badges when the list
/// is `width` wide.
fn line_columns(t: &Tokens, width: f32) -> usize {
    let inner = width - SCROLLBAR - 2.0 * PAD_X - 2.0 - gutter(t) - SPACING;
    let col = t.small() * 0.61;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = (inner / col).floor().max(8.0) as usize;
    n
}

/// Which list the refs panel shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RefsMode {
    #[default]
    Refs,
    Worktrees,
}

impl RefsMode {
    pub fn toggled(self) -> Self {
        match self {
            Self::Refs => Self::Worktrees,
            Self::Worktrees => Self::Refs,
        }
    }
}

/// One working tree and what was last read about it.
#[derive(Debug, Clone)]
pub struct Card {
    pub tree: Worktree,
    /// The repository's own working tree (the first one git lists).
    pub main: bool,
    pub status: Option<WorktreeStatus>,
    /// Why the status could not be read.
    pub error: Option<String>,
    /// The folder with symlinks resolved (read in the background), to
    /// match it with the tabs, which keep resolved paths.
    pub real: Option<PathBuf>,
    /// A gf tab shows this tree.
    pub in_tab: bool,
    /// The tab this view belongs to shows this tree.
    pub current: bool,
    /// Files this tree's work shares with other trees' (worked out once
    /// the statuses are read, see [`overlaps`]).
    pub overlap: Option<Overlap>,
}

/// Files a tree changes that other trees change too: merging both may
/// conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlap {
    /// Different files shared with any other tree.
    pub files: usize,
    /// One line per other tree: its name and the shared files.
    pub tip: String,
}

impl Card {
    fn name(&self) -> String {
        self.tree.branch.clone().unwrap_or_else(|| {
            self.tree.head.as_ref().map_or_else(
                || "(bare)".to_owned(),
                |h| format!("{} (detached)", h.short()),
            )
        })
    }

    /// The tree's folder exists and has a working tree to read.
    fn readable(&self) -> bool {
        !self.tree.prunable && !self.tree.bare
    }

    /// Ready to go: merged into the base, nothing uncommitted, not locked.
    fn finished(&self) -> bool {
        !self.main
            && !self.current
            && !self.tree.locked
            && self.readable()
            && self
                .status
                .as_ref()
                .is_some_and(|s| s.is_clean() && s.merged())
    }
}

/// What the remove dialog needs to know about a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub path: PathBuf,
    /// Branch, or `abc1234 (detached)`.
    pub name: String,
    /// Not on a branch: commits made there are kept by no branch.
    pub detached: bool,
    /// Uncommitted entries, when known.
    pub changed: Option<usize>,
}

#[derive(Debug, Default)]
pub struct State {
    /// In display order: the main tree, then by name.
    cards: Vec<Card>,
    /// The branch work goes into, as last read.
    base: Option<String>,
    selected: Option<PathBuf>,
    scroll: ScrollPos,
    /// Id of the latest status read; older results are dropped.
    request: u64,
    loading: bool,
    last_click: Option<(PathBuf, Instant)>,
    /// Folders gf tabs show, and the one of this view's tab.
    tabs: Vec<PathBuf>,
    here: PathBuf,
}

/// Statuses read in the background, for [`Message::Loaded`].
#[derive(Debug, Clone)]
pub struct Loaded {
    request: u64,
    base: Option<String>,
    /// Each tree's status and its resolved folder.
    statuses: Vec<(PathBuf, Result<WorktreeStatus, String>, Option<PathBuf>)>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// A click on a card; a second one on the same card opens it.
    Click(PathBuf),
    /// Keyboard selection (never counts as a click).
    Select(PathBuf),
    /// Right-click on a card: select it and open its menu.
    RightClick(PathBuf),
    /// Action on the selected card (buttons, keys, menu).
    Act(Action),
    /// `.`: the menu of the selected card.
    OpenActions,
    New,
    Prune,
    Cleanup,
    /// `«`: hide the panel.
    Collapse,
    Scrolled(ScrollPos),
    Loaded(Box<Loaded>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Compare,
    Editor,
    Terminal,
    Reveal,
    CopyPath,
    Lock,
    Unlock,
    Remove,
    /// Merge its branch into its base, in the tree that has the base.
    Merge,
    /// Choose the branch it goes back into.
    SetBase,
    /// Bring in the commits its base has and it lacks.
    Update,
}

/// Requests to the app. Removing and locking are confirmed there.
#[derive(Debug, Clone)]
pub enum Output {
    /// Show this revision's history (full ref name or commit id).
    ShowRevision(String),
    OpenRepo(PathBuf),
    /// The work done in a tree: its commits since it left the base.
    Compare {
        from: Oid,
        to: Oid,
        count: usize,
    },
    Editor(PathBuf),
    Terminal(PathBuf),
    Reveal(PathBuf),
    Copy(PathBuf),
    Lock(PathBuf, bool),
    Remove(Removal),
    /// Merge `branch` into `base`, checked out in the tree at `into`
    /// (`real`: that folder resolved, to find its tab). `from`: the tree
    /// `branch` is checked out in, when it can be removed afterwards.
    Merge {
        branch: String,
        base: String,
        into: PathBuf,
        real: Option<PathBuf>,
        count: u32,
        from: Option<PathBuf>,
    },
    /// Bring the `behind` commits of `base` into `branch`, checked out in
    /// the tree at `tree` (`real`: resolved, to find its tab).
    Update {
        branch: String,
        base: String,
        tree: PathBuf,
        real: Option<PathBuf>,
        behind: u32,
    },
    /// Ask which branch `branch` goes back into; `base` is the current one.
    SetBase {
        branch: String,
        base: Option<String>,
        guessed: bool,
    },
    /// Trees merged into `base`, with their branches.
    Cleanup {
        base: String,
        trees: Vec<(PathBuf, Option<String>)>,
    },
    Prune,
    New,
    /// Hide the refs panel (⌘B).
    Collapse,
    /// A card's menu; `show` is the history to show first (the card was
    /// just selected by the right-click).
    Menu {
        show: Option<String>,
        menu: Menu<Message>,
    },
}

impl State {
    pub fn cards(&self) -> &[Card] {
        &self.cards
    }

    /// The branch work goes into, as last read.
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    pub fn selected(&self) -> Option<&Card> {
        let sel = self.selected.as_ref()?;
        self.cards.iter().find(|c| &c.tree.path == sel)
    }

    /// Takes the repository's worktree list (from a refresh), keeping what
    /// was read about trees that are still there.
    pub fn set_worktrees(&mut self, list: &[Worktree]) {
        let mut old: std::collections::HashMap<PathBuf, Card> = self
            .cards
            .drain(..)
            .map(|c| (c.tree.path.clone(), c))
            .collect();
        self.cards = list
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let prev = old.remove(&w.path);
                Card {
                    tree: w.clone(),
                    main: i == 0,
                    status: prev.as_ref().and_then(|c| c.status.clone()),
                    error: prev.as_ref().and_then(|c| c.error.clone()),
                    overlap: prev.as_ref().and_then(|c| c.overlap.clone()),
                    real: prev.and_then(|c| c.real),
                    in_tab: false,
                    current: false,
                }
            })
            .collect();
        self.cards.sort_by(|a, b| {
            b.main
                .cmp(&a.main)
                .then_with(|| a.name().to_lowercase().cmp(&b.name().to_lowercase()))
                .then_with(|| a.tree.path.cmp(&b.tree.path))
        });
        self.mark_tabs();
        if self
            .selected
            .as_ref()
            .is_some_and(|s| !self.cards.iter().any(|c| &c.tree.path == s))
        {
            self.selected = None;
        }
    }

    /// The folders gf tabs show (resolved paths) and `here`, the one of the
    /// tab this view belongs to.
    pub fn set_tabs(&mut self, open: Vec<PathBuf>, here: PathBuf) {
        self.tabs = open;
        self.here = here;
        self.mark_tabs();
    }

    fn mark_tabs(&mut self) {
        for c in &mut self.cards {
            // A tab may show the folder as git names it or resolved.
            let is = |p: &PathBuf| *p == c.tree.path || c.real.as_ref() == Some(p);
            c.in_tab = self.tabs.iter().any(is);
            c.current = is(&self.here);
        }
        // With nothing picked, the tree this tab shows is the one marked.
        if self.selected.is_none() {
            self.selected = self
                .cards
                .iter()
                .find(|c| c.current)
                .map(|c| c.tree.path.clone());
        }
    }

    /// Drops the selection so the tree this tab shows is marked again
    /// (when the tab is opened or switched to from another tree).
    pub fn select_here(&mut self) {
        self.selected = None;
        self.mark_tabs();
    }

    /// Reads every tree's status in the background. A read already running
    /// is left to finish: its result is current enough, and the next
    /// refresh or poll comes soon.
    pub fn request(&mut self, repo: &Repository) -> Task<Message> {
        if self.loading {
            return Task::none();
        }
        let paths: Vec<PathBuf> = self
            .cards
            .iter()
            .filter(|c| c.readable())
            .map(|c| c.tree.path.clone())
            .collect();
        if paths.is_empty() {
            return Task::none();
        }
        self.loading = true;
        self.request += 1;
        let request = self.request;
        let repo = repo.clone();
        Task::perform(
            async move {
                let resolve = {
                    let paths = paths.clone();
                    tokio::task::spawn_blocking(move || {
                        paths
                            .iter()
                            .map(|p| crate::platform::canonicalize(p))
                            .collect::<Vec<_>>()
                    })
                };
                let base = repo.default_base().await.ok().flatten();
                let reads = paths.iter().map(|p| {
                    let (repo, base) = (&repo, base.as_deref());
                    async move {
                        repo.worktree_status(p, base)
                            .await
                            .map_err(|e| e.to_string())
                    }
                });
                let statuses = iced::futures::future::join_all(reads).await;
                let real = resolve.await.unwrap_or_default();
                Loaded {
                    request,
                    base,
                    statuses: paths
                        .into_iter()
                        .zip(statuses)
                        .enumerate()
                        .map(|(i, (p, s))| (p, s, real.get(i).cloned().flatten()))
                        .collect(),
                }
            },
            |l| Message::Loaded(Box::new(l)),
        )
    }

    fn loaded(&mut self, l: Loaded) {
        if l.request != self.request {
            return;
        }
        self.loading = false;
        self.base = l.base;
        for (path, status, real) in l.statuses {
            if let Some(c) = self.cards.iter_mut().find(|c| c.tree.path == path) {
                match status {
                    Ok(s) => {
                        c.status = Some(s);
                        c.error = None;
                    }
                    Err(e) => c.error = Some(e),
                }
                if real.is_some() {
                    c.real = real;
                }
            }
        }
        let touched: Vec<Option<&[PathBuf]>> = self
            .cards
            .iter()
            .map(|c| c.status.as_ref().and_then(|s| s.touched.as_deref()))
            .collect();
        let names: Vec<String> = self.cards.iter().map(Card::name).collect();
        let found = overlaps(&touched, &names);
        for (c, o) in self.cards.iter_mut().zip(found) {
            c.overlap = o;
        }
        self.mark_tabs();
    }

    fn index_of(&self, path: &Path) -> Option<usize> {
        self.cards.iter().position(|c| c.tree.path == path)
    }

    /// What selecting a card shows: its branch's history.
    fn show(card: &Card) -> Option<String> {
        match (&card.tree.branch, &card.tree.head) {
            (Some(b), _) => Some(format!("refs/heads/{b}")),
            (None, Some(h)) => Some(h.to_string()),
            (None, None) => None,
        }
    }

    /// Selects the card at `path`; the history to show when it changed.
    fn select(&mut self, path: PathBuf) -> Option<String> {
        if self.selected.as_ref() == Some(&path) {
            return None;
        }
        self.selected = Some(path);
        self.selected().and_then(Self::show)
    }

    fn output_for(&self, action: Action) -> Option<Output> {
        let card = self.selected()?;
        let path = card.tree.path.clone();
        let readable = card.readable();
        Some(match action {
            Action::Open if !card.current && readable => Output::OpenRepo(path),
            Action::Compare => {
                let s = card.status.as_ref()?;
                let to = s
                    .last_commit
                    .as_ref()
                    .map(|c| c.id.clone())
                    .or_else(|| card.tree.head.clone())?;
                Output::Compare {
                    from: s.merge_base.clone().filter(|_| s.ahead_of_base > 0)?,
                    to,
                    count: usize::try_from(s.ahead_of_base).unwrap_or(usize::MAX),
                }
            }
            Action::Editor if readable => Output::Editor(path),
            Action::Terminal if readable => Output::Terminal(path),
            Action::Reveal if readable => Output::Reveal(path),
            Action::CopyPath => Output::Copy(path),
            Action::Lock if !card.main && !card.tree.locked => Output::Lock(path, true),
            Action::Unlock if card.tree.locked => Output::Lock(path, false),
            // A tree whose folder is gone is forgotten, not removed.
            Action::Remove if card.tree.prunable && !card.tree.locked => Output::Prune,
            Action::Merge => {
                let branch = card.tree.branch.clone()?;
                let s = card.status.as_ref()?;
                let base = s.base.clone().filter(|_| s.ahead_of_base > 0)?;
                let into = self.tree_of(&base)?;
                Output::Merge {
                    branch,
                    base,
                    into: into.tree.path.clone(),
                    real: into.real.clone(),
                    count: s.ahead_of_base,
                    from: (!card.main && !card.tree.locked).then(|| path.clone()),
                }
            }
            Action::Update if readable => {
                let branch = card.tree.branch.clone()?;
                let s = card.status.as_ref()?;
                let base = s
                    .base
                    .clone()
                    .filter(|b| s.behind_base > 0 && *b != branch)?;
                Output::Update {
                    branch,
                    base,
                    tree: path,
                    real: card.real.clone(),
                    behind: s.behind_base,
                }
            }
            // The default base itself goes nowhere.
            Action::SetBase if card.tree.branch.is_some() && card.tree.branch != self.base => {
                let s = card.status.as_ref();
                Output::SetBase {
                    branch: card.tree.branch.clone()?,
                    base: s.and_then(|s| s.base.clone()),
                    guessed: s.is_some_and(|s| s.base_guessed),
                }
            }
            Action::Remove if !card.main && !card.current && !card.tree.locked => {
                Output::Remove(Removal {
                    path,
                    name: card.name(),
                    detached: card.tree.branch.is_none(),
                    changed: card.status.as_ref().map(WorktreeStatus::changed),
                })
            }
            _ => return None,
        })
    }

    /// The tree that has `branch` checked out (and still has its folder).
    fn tree_of(&self, branch: &str) -> Option<&Card> {
        self.cards
            .iter()
            .find(|c| c.tree.branch.as_deref() == Some(branch) && !c.tree.prunable)
    }

    /// The selected card's menu; only what applies to it.
    fn menu(&self) -> Option<Menu<Message>> {
        let card = self.selected()?;
        let mut entries = Vec::new();
        let mut add = |key, label: &str, hint: &str, a: Action| {
            if self.output_for(a).is_some() {
                entries.push(Entry::item(key, label, hint, Message::Act(a)));
            }
        };
        add(
            'o',
            "open in tab",
            "work on it in its own tab",
            Action::Open,
        );
        add(
            'c',
            "compare with base",
            "what it changed since it branched off",
            Action::Compare,
        );
        let base = card.status.as_ref().and_then(|s| s.base.clone());
        if let Some(base) = &base {
            let update = format!("update from {base}…");
            add(
                'u',
                &update,
                "bring in the commits it lacks",
                Action::Update,
            );
            let merge = format!("merge into {base}…");
            add(
                'm',
                &merge,
                "runs where the base is checked out",
                Action::Merge,
            );
        }
        add(
            'b',
            "base branch…",
            "the branch it merges into",
            Action::SetBase,
        );
        add(
            'e',
            "open in IDE",
            "the folder in your editor (:editor)",
            Action::Editor,
        );
        let terminal = format!("open in {}", crate::platform::TERMINAL);
        add(
            't',
            &terminal,
            "a shell in this folder (:terminal)",
            Action::Terminal,
        );
        let reveal = format!("reveal in {}", crate::platform::FILE_MANAGER);
        add('r', &reveal, "show the folder", Action::Reveal);
        add('y', "copy path", "the folder's full path", Action::CopyPath);
        add(
            'l',
            "lock",
            "protect it from remove and prune",
            Action::Lock,
        );
        add('l', "unlock", "allow removing it again", Action::Unlock);
        if self.output_for(Action::Remove).is_some() {
            let (label, hint) = if card.tree.prunable {
                ("prune…", "clean up worktrees whose folder was deleted")
            } else {
                ("remove…", "delete its folder; the branch stays")
            };
            entries.push(Entry::Separator);
            entries.push(Entry::caution(
                'd',
                label,
                hint,
                Message::Act(Action::Remove),
            ));
        }
        (!entries.is_empty()).then(|| Menu {
            title: card.name(),
            entries,
        })
    }

    /// Trees that can go: merged, clean, unlocked.
    fn finished(&self) -> Vec<(PathBuf, Option<String>)> {
        self.cards
            .iter()
            .filter(|c| c.finished())
            .map(|c| (c.tree.path.clone(), c.tree.branch.clone()))
            .collect()
    }

    fn has_missing(&self) -> bool {
        self.cards.iter().any(|c| c.tree.prunable)
    }

    pub fn nav(&mut self, nav: Nav, t: &Tokens) -> Option<Moved<Message>> {
        let act = |a| Some(Moved::message(Message::Act(a)));
        match nav.key {
            Key::Enter | Key::Space => return act(Action::Open),
            Key::Char('c') => return act(Action::Compare),
            Key::Char('e') => return act(Action::Editor),
            Key::Char('t') => return act(Action::Terminal),
            Key::Char('r') => return act(Action::Reveal),
            Key::Char('d') => return act(Action::Remove),
            Key::Char('y') => return act(Action::CopyPath),
            Key::Char('m') => return act(Action::Merge),
            Key::Char('u') => return act(Action::Update),
            Key::Char('b') => return act(Action::SetBase),
            Key::Char('l') => {
                let Some(card) = self.selected() else {
                    return Some(Moved::stay());
                };
                return act(if card.tree.locked {
                    Action::Unlock
                } else {
                    Action::Lock
                });
            }
            Key::Char('n') => return Some(Moved::message(Message::New)),
            Key::Char('.') => {
                return Some(
                    self.selected
                        .as_ref()
                        .map_or_else(Moved::stay, |_| Moved::message(Message::OpenActions)),
                );
            }
            Key::Left | Key::Right | Key::Tab | Key::Char(_) => return None,
            _ => {}
        }
        let h = row_height(t);
        let current = self.selected.as_deref().and_then(|p| self.index_of(p));
        let i = nav::step(
            current,
            self.cards.len(),
            nav.key,
            nav::page(self.scroll, h),
        )?;
        if current == Some(i) {
            return Some(Moved::stay());
        }
        let path = self.cards[i].tree.path.clone();
        Some(Moved::message(Message::Select(path)).with_task(nav::reveal(
            LIST_ID,
            self.scroll,
            i,
            h,
        )))
    }

    /// One card, its text cut to `cols` small-size columns per line.
    fn card<'a>(
        &'a self,
        t: &'a Tokens,
        c: &'a Card,
        cols: usize,
        now: SystemTime,
    ) -> Element<'a, Message> {
        let selected = self.selected.as_ref() == Some(&c.tree.path);
        let (big, small) = line_heights(t);
        let badge = |s: String, color| colored(t, s, color).size(t.small());
        let base = c
            .status
            .as_ref()
            .and_then(|s| s.base.as_deref())
            .or(self.base.as_deref());

        // Line 1: mark, name, lock, tab, and what the tree is up to.
        let mut tags: Vec<(Element<'a, Message>, usize)> = Vec::new();
        if c.tree.locked {
            tags.push((
                widgets::tip(
                    t,
                    badge("locked".into(), t.warning()),
                    "can't be removed until unlocked · l unlocks".into(),
                ),
                6,
            ));
        }
        if let Some(p) = phase(c, now) {
            let (word, color) = match p {
                Phase::Missing | Phase::Error | Phase::Conflict => (p.word(), t.error()),
                Phase::Editing | Phase::Uncommitted => (p.word(), t.warning()),
                Phase::Merged => (p.word(), t.success()),
                Phase::Ready | Phase::Behind | Phase::Clean => (p.word(), t.dim),
            };
            let mut text = word.to_owned();
            if p == Phase::Editing
                && let Some(ago) = c
                    .status
                    .as_ref()
                    .and_then(|s| s.last_activity)
                    .and_then(|at| now.duration_since(at).ok())
            {
                text = format!("{word} {}", format::age(ago));
            }
            let n_cols = columns(&text);
            tags.push((
                widgets::tip(t, badge(text, color), p.tip(base.unwrap_or("the base"))),
                n_cols,
            ));
        }
        let used: usize = tags.iter().map(|(_, n)| n + 1).sum();
        let name_cols = scale_columns(cols.saturating_sub(used + 1), t.small() / t.font_size);
        let name_color = t.branch();
        // The tree on the base heads the others: no title, nothing to hang
        // from.
        let on_base = self.base.is_some() && c.tree.branch == self.base;
        let elbow = || colored(t, if on_base { " " } else { "└" }, t.dim).wrapping(Wrapping::None);
        let mark: Element<'a, Message> = if c.current {
            widgets::tip(
                t,
                row![elbow(), colored(t, "●", t.head())],
                "open in this tab".into(),
            )
        } else if c.in_tab {
            widgets::tip(
                t,
                row![elbow(), colored(t, "◉", t.accent())],
                "open in another tab".into(),
            )
        } else if on_base {
            space::horizontal().into()
        } else {
            colored(t, "└─", t.dim).wrapping(Wrapping::None).into()
        };
        let name = c.name();
        let shown = format::truncate_columns(&name, name_cols);
        let where_tip = format!("{} · right-click for actions", format::tilde(&c.tree.path));
        // A name cut with `…` shows whole in the tooltip.
        let name_tip = if shown == name {
            where_tip
        } else {
            format!("{name}\n{where_tip}")
        };
        let first = line(
            t,
            big,
            mark,
            widgets::tip(
                t,
                colored(t, shown, name_color).wrapping(Wrapping::None),
                name_tip,
            ),
            tags.into_iter().map(|(e, _)| e).collect(),
        );

        let detail = |parts: Vec<(String, Part)>, tip: String| {
            let text: Element<'a, Message> =
                Row::with_children(fit_parts(parts, cols).into_iter().map(|(s, part)| {
                    colored(t, s, part_color(t, part))
                        .size(t.small())
                        .wrapping(Wrapping::None)
                        .into()
                }))
                .into();
            let body = if tip.is_empty() {
                text
            } else {
                widgets::tip(t, text, tip)
            };
            line(t, small, space::horizontal().into(), body, Vec::new())
        };

        // Line 2: what is uncommitted there, and files other trees change.
        let mut tip = Vec::new();
        if let Some(s) = &c.status {
            tip.push(format!(
                "{} staged · {} unstaged · {} untracked · {} conflicted",
                s.staged, s.unstaged, s.untracked, s.conflicts
            ));
        }
        if let Some(e) = &c.error {
            tip.push(format::truncate(e, 200));
        }
        if let Some(o) = &c.overlap {
            tip.push(o.tip.clone());
        }
        let second = detail(changes_parts(c), tip.join("\n"));

        // Line 4: the upstream and the last commit.
        let (parts, tip) = match &c.status {
            Some(s) => {
                let parts = remote_parts(s, now);
                let mut tip = sync_tip(s);
                if let Some(lc) = &s.last_commit {
                    tip.push(format!(
                        "last commit {} {}",
                        lc.id.short(),
                        format::truncate(&lc.subject, 120)
                    ));
                }
                (parts, tip.join("\n"))
            }
            None => (Vec::new(), String::new()),
        };
        let fourth = detail(parts, tip);

        // Title: the branch it goes into and how it stands against it.
        let parts = title_parts(c);
        let shown: Element<'a, Message> =
            Row::with_children(fit_parts(parts, title_columns(t, cols)).into_iter().map(
                |(s, part)| {
                    colored(t, s, part_color(t, part))
                        .size(t.small())
                        .wrapping(Wrapping::None)
                        .into()
                },
            ))
            .into();
        let tip = match &c.status {
            Some(s) if s.base.is_some() => self.base_tip(s),
            Some(_) => "b sets the branch this one merges into".to_owned(),
            None => String::new(),
        };
        let title = container(if tip.is_empty() {
            shown
        } else {
            widgets::tip(t, shown, tip)
        })
        .width(Length::Fill)
        .height(Length::Fixed(small))
        .align_y(Alignment::Center)
        .clip(true);

        // Every card is as tall, for the list: the tree on the base keeps
        // the title's line as space below.
        let lines = if on_base {
            column![first, second, fourth, space::vertical().height(small)]
        } else {
            column![title, first, second, fourth]
        };
        let body = button(lines)
            .width(Length::Fill)
            .padding(Padding::from([PAD_Y, PAD_X]))
            .style(style::card_button(t, selected))
            .on_press(Message::Click(c.tree.path.clone()));
        container(mouse_area(body).on_right_press(Message::RightClick(c.tree.path.clone())))
            .padding(Padding {
                bottom: GAP,
                ..Padding::ZERO
            })
            .into()
    }

    /// Tooltip of the base line: what the numbers mean and the keys that
    /// act on them.
    fn base_tip(&self, s: &WorktreeStatus) -> String {
        let Some(base) = &s.base else {
            return String::new();
        };
        let mut tip = Vec::new();
        if s.merged() {
            tip.push(format!("all merged into {base} · safe to remove"));
        } else if s.ahead_of_base > 0 {
            tip.push(format!(
                "{} not in {base} yet · c compares",
                format::count(s.ahead_of_base, "commit")
            ));
        }
        if s.behind_base > 0 && !s.merged() {
            tip.push(format!(
                "{base} has {} it lacks · u brings them in",
                format::count(s.behind_base, "commit")
            ));
        }
        if s.ahead_of_base > 0 && self.tree_of(base).is_some() {
            tip.push(format!("m merges it into {base}"));
        }
        tip.push(if s.base_guessed {
            format!("{base} is a guess · b sets the real one")
        } else {
            "b changes the base".to_owned()
        });
        tip.join("\n")
    }
}

/// What a tree is up to, in one word at the end of its first line. The
/// first that applies wins: problems, then work in progress, then where
/// the work stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Missing,
    Error,
    Conflict,
    Editing,
    Uncommitted,
    Merged,
    Ready,
    Behind,
    Clean,
}

impl Phase {
    fn word(self) -> &'static str {
        match self {
            Self::Missing => "MISSING",
            Self::Error => "ERROR",
            Self::Conflict => "CONFLICT",
            Self::Editing => "EDITING",
            Self::Uncommitted => "UNCOMMITTED",
            Self::Merged => "MERGED",
            Self::Ready => "READY",
            Self::Behind => "BEHIND",
            Self::Clean => "CLEAN",
        }
    }

    fn tip(self, base: &str) -> String {
        match self {
            Self::Missing => "the folder was deleted · prune cleans it up".into(),
            Self::Error => "its status could not be read".into(),
            Self::Conflict => "files have conflicts to resolve".into(),
            Self::Editing => "files changed in the last 2 minutes: someone is working here".into(),
            Self::Uncommitted => "changes not committed yet".into(),
            Self::Merged => format!("all its work is in {base} · d removes it"),
            Self::Ready => format!("committed work not in {base} yet · m merges it"),
            Self::Behind => format!("{base} moved on · u brings its commits in"),
            Self::Clean => "nothing to commit or merge".into(),
        }
    }
}

/// The card's [`Phase`], `None` while its status is being read.
fn phase(c: &Card, now: SystemTime) -> Option<Phase> {
    if c.tree.prunable {
        return Some(Phase::Missing);
    }
    if c.error.is_some() {
        return Some(Phase::Error);
    }
    let s = c.status.as_ref()?;
    Some(if s.conflicts > 0 {
        Phase::Conflict
    } else if !s.is_clean()
        && s.last_activity
            .and_then(|at| now.duration_since(at).ok())
            .is_some_and(|ago| ago < ACTIVE_FOR)
    {
        Phase::Editing
    } else if !s.is_clean() {
        Phase::Uncommitted
    } else if s.merged() {
        Phase::Merged
    } else if s.base.is_some() && s.ahead_of_base > 0 {
        Phase::Ready
    } else if s.base.is_some() && s.behind_base > 0 {
        Phase::Behind
    } else {
        Phase::Clean
    })
}

/// What a piece of a card's title or detail lines says, for its color.
/// Most is dim: color is kept for what needs attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Dim,
    /// The base's name in the title.
    Strong,
    /// Files other trees change too.
    Warn,
    Bad,
    /// The last commit's subject: cut with `…` rather than dropped.
    Subject,
}

fn part_color(t: &Tokens, part: Part) -> iced::Color {
    match part {
        Part::Dim | Part::Subject => t.dim,
        Part::Strong => t.fg,
        Part::Warn => t.warning(),
        Part::Bad => t.error(),
    }
}

const SEP: &str = " · ";
/// Between the base and how the tree stands against it in the title.
const ARROW: &str = " ← ";

/// Line 2: `3 uncommitted · ⚠ 2 shared`, `1 conflict`, `clean`.
fn changes_parts(c: &Card) -> Vec<(String, Part)> {
    if c.tree.prunable {
        return vec![("folder deleted".into(), Part::Bad)];
    }
    if let Some(e) = &c.error {
        let first = e.lines().next().unwrap_or_default();
        return vec![(first.to_owned(), Part::Bad)];
    }
    let Some(s) = &c.status else {
        return vec![("…".into(), Part::Dim)];
    };
    let mut parts = Vec::new();
    let mut push = |text: String, part| {
        if !parts.is_empty() {
            parts.push((SEP.into(), Part::Dim));
        }
        parts.push((text, part));
    };
    if s.conflicts > 0 {
        push(
            format::count(u64::try_from(s.conflicts).unwrap_or(u64::MAX), "conflict"),
            Part::Bad,
        );
    }
    let changed = s.changed() - s.conflicts;
    if changed > 0 {
        push(format!("{changed} uncommitted"), Part::Dim);
    }
    if s.conflicts == 0 && changed == 0 {
        push("clean".into(), Part::Dim);
    }
    if let Some(o) = &c.overlap {
        push(format!("⚠ {} shared", o.files), Part::Warn);
    }
    parts
}

/// The title above a card, about the branch the tree merges into:
/// `main ← 3 to merge · 3 new`. Whether the base is a guess is in its
/// tooltip. The tree on the base itself has no title.
fn title_parts(c: &Card) -> Vec<(String, Part)> {
    let dim = |s: &str| vec![(s.to_owned(), Part::Dim)];
    if c.tree.prunable {
        return dim("folder deleted · prune cleans it up");
    }
    if c.error.is_some() {
        return dim("status unknown");
    }
    let Some(s) = &c.status else {
        return dim("…");
    };
    let Some(base) = &s.base else {
        return dim("no base · b sets one");
    };
    let mut parts = vec![(base.clone(), Part::Strong), (ARROW.into(), Part::Dim)];
    if s.last_commit.is_none() {
        parts.push(("no commits yet".into(), Part::Dim));
    } else if s.merged() {
        parts.push(("all merged".into(), Part::Dim));
    } else {
        let start = parts.len();
        if s.ahead_of_base > 0 {
            parts.push((format!("{} to merge", s.ahead_of_base), Part::Dim));
        }
        if s.behind_base > 0 {
            if parts.len() > start {
                parts.push((SEP.into(), Part::Dim));
            }
            parts.push((format!("{} new", s.behind_base), Part::Dim));
        }
        if parts.len() == start {
            parts.push(("up to date".into(), Part::Dim));
        }
    }
    parts
}

/// The arrows of the last line in words, for its tooltip.
fn sync_tip(s: &WorktreeStatus) -> Vec<String> {
    let Some(up) = &s.upstream else {
        return vec!["⊘ never pushed: no upstream branch".into()];
    };
    let mut tip = Vec::new();
    if s.ahead > 0 {
        tip.push(format!(
            "↑{} to push to {up}",
            format::count(s.ahead, "commit")
        ));
    }
    if s.behind > 0 {
        tip.push(format!(
            "↓{} to pull from {up}",
            format::count(s.behind, "commit")
        ));
    }
    if tip.is_empty() {
        tip.push(format!("in sync with {up}"));
    }
    tip
}

/// The last line: the upstream distance in arrows and the last commit,
/// `↑2 ↓1 · Add refund flow 15m`; `⊘` for no upstream, nothing when in
/// sync. The tooltip says it in words.
fn remote_parts(s: &WorktreeStatus, now: SystemTime) -> Vec<(String, Part)> {
    let mut parts = Vec::new();
    let sync = match (&s.upstream, s.ahead, s.behind) {
        (None, ..) => Some("⊘".to_owned()),
        (Some(_), 0, 0) => None,
        (Some(_), a, 0) => Some(format!("↑{a}")),
        (Some(_), 0, b) => Some(format!("↓{b}")),
        (Some(_), a, b) => Some(format!("↑{a} ↓{b}")),
    };
    if let Some(sync) = sync {
        parts.push((sync, Part::Dim));
    }
    if let Some(lc) = &s.last_commit {
        let age = u64::try_from(lc.time)
            .ok()
            .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .and_then(|at| now.duration_since(at).ok())
            .map_or_else(String::new, |d| format!(" {}", format::age(d)));
        if !parts.is_empty() {
            parts.push((SEP.into(), Part::Dim));
        }
        parts.push((format!("{}{age}", lc.subject), Part::Subject));
    }
    parts
}

/// Columns of the title, which is not indented: those of the other lines
/// (`cols`) and the indent's.
fn title_columns(t: &Tokens, cols: usize) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let indent = ((gutter(t) + SPACING) / (t.small() * 0.61)).floor() as usize;
    cols + indent
}

/// `parts` cut to `room` columns: a commit subject is shortened with `…`
/// while a few columns of it fit; other trailing `· x` pieces drop whole,
/// and the first piece is shortened last.
fn fit_parts(mut parts: Vec<(String, Part)>, room: usize) -> Vec<(String, Part)> {
    /// Least of a subject worth showing.
    const MIN_SUBJECT: usize = 6;
    let width = |parts: &[(String, Part)]| parts.iter().map(|(s, _)| columns(s)).sum::<usize>();
    loop {
        let used = width(&parts);
        if used <= room {
            return parts;
        }
        let over = used - room;
        let only = parts.len() == 1;
        let Some((last, part)) = parts.last_mut() else {
            return parts;
        };
        let keep = columns(last).saturating_sub(over);
        if only || (*part == Part::Subject && keep >= MIN_SUBJECT) {
            *last = format::truncate_columns(last, keep);
            return parts;
        }
        // Drop the piece and the separator before it.
        parts.pop();
        if parts.last().is_some_and(|(s, _)| s == SEP || s == ARROW) {
            parts.pop();
        }
    }
}

/// `n` columns of one font size in columns of another (`ratio` = their
/// size ratio).
fn scale_columns(n: usize, ratio: f32) -> usize {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let scaled = (n as f32 * ratio).floor().max(0.0) as usize;
    scaled
}

/// A card line of fixed `height`: the mark column, `body` (cut, never
/// pushing), and the badges at the right end.
fn line<'a>(
    t: &Tokens,
    height: f32,
    mark: Element<'a, Message>,
    body: Element<'a, Message>,
    badges: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    row![
        container(mark).width(Length::Fixed(gutter(t))),
        container(body).width(Length::Fill).clip(true),
        Row::with_children(badges)
            .spacing(6)
            .align_y(Alignment::Center),
    ]
    .spacing(SPACING)
    .align_y(Alignment::Center)
    .height(Length::Fixed(height))
    .into()
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message {
            Message::Click(path) => {
                let now = Instant::now();
                let double = self
                    .last_click
                    .as_ref()
                    .is_some_and(|(p, at)| *p == path && now.duration_since(*at) < DOUBLE_CLICK);
                if double {
                    self.last_click = None;
                    return self
                        .output_for(Action::Open)
                        .map_or_else(Update::none, Update::output);
                }
                self.last_click = Some((path.clone(), now));
                self.select(path)
                    .map_or_else(Update::none, |r| Update::output(Output::ShowRevision(r)))
            }
            Message::Select(path) => {
                self.last_click = None;
                self.select(path)
                    .map_or_else(Update::none, |r| Update::output(Output::ShowRevision(r)))
            }
            Message::RightClick(path) => {
                self.last_click = None;
                let show = self.select(path);
                match self.menu() {
                    Some(menu) => Update::output(Output::Menu { show, menu }),
                    None => {
                        show.map_or_else(Update::none, |r| Update::output(Output::ShowRevision(r)))
                    }
                }
            }
            Message::OpenActions => self.menu().map_or_else(Update::none, |menu| {
                Update::output(Output::Menu { show: None, menu })
            }),
            Message::Act(a) => self.output_for(a).map_or_else(Update::none, Update::output),
            Message::New => Update::output(Output::New),
            Message::Prune => Update::output(Output::Prune),
            Message::Collapse => Update::output(Output::Collapse),
            Message::Cleanup => {
                let trees = self.finished();
                match self.base.clone() {
                    Some(base) if !trees.is_empty() => {
                        Update::output(Output::Cleanup { base, trees })
                    }
                    _ => Update::none(),
                }
            }
            Message::Scrolled(pos) => {
                self.scroll = pos;
                Update::none()
            }
            Message::Loaded(l) => {
                self.loaded(*l);
                Update::none()
            }
        }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let finished = self.cards.iter().filter(|c| c.finished()).count();
        let mut actions = row![].spacing(4).align_y(Alignment::Center);
        if self.has_missing() {
            actions = actions.push(widgets::tip(
                t,
                widgets::text_button(t, "prune", None, Some(Message::Prune)),
                "clean up worktrees whose folder was deleted".to_owned(),
            ));
        }
        if finished > 0 {
            actions = actions.push(widgets::tip(
                t,
                widgets::text_button(
                    t,
                    &format!("clean {finished}"),
                    None,
                    Some(Message::Cleanup),
                ),
                format!(
                    "remove worktrees merged into {}",
                    self.base.as_deref().unwrap_or("the base")
                ),
            ));
        }
        actions = actions.push(widgets::icon_button(
            t,
            "+",
            t.accent(),
            "new worktree  n",
            Message::New,
        ));
        actions = actions.push(widgets::icon_button(
            t,
            "«",
            t.accent(),
            "hide worktrees ⌘B",
            Message::Collapse,
        ));
        let title = "WORKTREES".to_owned();
        if cx.snapshot.is_none() {
            return widgets::panel_with_action(
                t,
                title,
                actions,
                widgets::empty_hint(t, "loading…"),
            );
        }
        let alone = self.cards.len() <= 1;
        let list = responsive(move |size| {
            let cols = line_columns(t, size.width);
            let now = SystemTime::now();
            if alone {
                return column![
                    self.cards.first().map_or_else(
                        || space::vertical().height(0).into(),
                        |c| { self.card(t, c, cols, now) }
                    ),
                    widgets::empty_hint(t, "no other worktrees — + (n) adds one"),
                ]
                .into();
            }
            virtual_list(
                LIST_ID,
                self.cards.len(),
                row_height(t),
                0.0,
                self.scroll,
                move |i| self.card(t, &self.cards[i], cols, now),
                Message::Scrolled,
            )
        });
        widgets::panel_with_action(t, title, actions, list)
    }
}

/// Shared files shown per other tree before `(+n)`.
const OVERLAP_SHOWN: usize = 3;

/// For each tree (`touched`: its sorted files, `None` when unknown), the
/// files other trees touch too, by tree. An untracked folder (`dir/`)
/// covers what is under it.
fn overlaps(touched: &[Option<&[PathBuf]>], names: &[String]) -> Vec<Option<Overlap>> {
    touched
        .iter()
        .enumerate()
        .map(|(i, mine)| {
            let mine = (*mine)?;
            let mut all: Vec<&Path> = Vec::new();
            let mut lines = Vec::new();
            for (j, theirs) in touched.iter().enumerate() {
                let Some(theirs) = theirs.filter(|_| j != i) else {
                    continue;
                };
                let shared = shared_files(mine, theirs);
                if shared.is_empty() {
                    continue;
                }
                let shown: Vec<String> = shared
                    .iter()
                    .take(OVERLAP_SHOWN)
                    .map(|p| p.display().to_string())
                    .collect();
                let more = match shared.len().saturating_sub(OVERLAP_SHOWN) {
                    0 => String::new(),
                    n => format!(" (+{n})"),
                };
                lines.push(format!("{}: {}{more}", names[j], shown.join(", ")));
                all.extend(shared);
            }
            all.sort_unstable();
            all.dedup();
            (!all.is_empty()).then(|| {
                lines.insert(0, "other worktrees change these files too".into());
                lines.push("merging both may conflict".into());
                Overlap {
                    files: all.len(),
                    tip: lines.join("\n"),
                }
            })
        })
        .collect()
}

/// Files of `mine` that `theirs` has too (both sorted); a folder entry
/// (`dir/`) on either side matches the files under it.
fn shared_files<'a>(mine: &'a [PathBuf], theirs: &'a [PathBuf]) -> Vec<&'a Path> {
    let is_dir = |p: &Path| p.as_os_str().to_string_lossy().ends_with('/');
    // The entries of `list` that are `dir` or under it.
    let under = |list: &'a [PathBuf], dir: &Path| -> &'a [PathBuf] {
        let start = list.partition_point(|p| p.as_path() < dir);
        let len = list[start..]
            .iter()
            .take_while(|p| p.starts_with(dir))
            .count();
        &list[start..start + len]
    };
    // Untracked folders are few: gathered once, not per file.
    let their_dirs: Vec<&Path> = theirs
        .iter()
        .map(PathBuf::as_path)
        .filter(|p| is_dir(p))
        .collect();
    let mut out: Vec<&Path> = Vec::new();
    for p in mine {
        let hit = if is_dir(p) {
            !under(theirs, p).is_empty()
        } else {
            theirs.binary_search(p).is_ok()
                || (!their_dirs.is_empty()
                    && p.ancestors().skip(1).any(|a| their_dirs.contains(&a)))
        };
        if hit {
            out.push(p.as_path());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gf_git::LastCommit;

    fn oid(c: char) -> Oid {
        c.to_string().repeat(40).parse().expect("oid")
    }

    fn tree(path: &str, branch: &str) -> Worktree {
        Worktree {
            path: path.into(),
            head: Some(oid('a')),
            branch: Some(branch.into()),
            bare: false,
            locked: false,
            prunable: false,
        }
    }

    fn status(commit_time: i64) -> WorktreeStatus {
        WorktreeStatus {
            last_commit: Some(LastCommit {
                id: oid('a'),
                subject: "x".into(),
                time: commit_time,
            }),
            ..WorktreeStatus::default()
        }
    }

    fn loaded(s: &mut State, statuses: Vec<(&str, WorktreeStatus)>) {
        s.request += 1;
        s.loading = true;
        let request = s.request;
        let _ = s.update(Message::Loaded(Box::new(Loaded {
            request,
            base: Some("main".into()),
            statuses: statuses
                .into_iter()
                .map(|(p, st)| (PathBuf::from(p), Ok(st), None))
                .collect(),
        })));
    }

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    fn state() -> State {
        let mut s = State::default();
        s.set_worktrees(&[tree("/r", "main"), tree("/b", "b"), tree("/a", "a")]);
        s.set_tabs(vec!["/r".into()], "/r".into());
        s
    }

    #[test]
    fn main_tree_first_then_by_name_whatever_the_activity() {
        let mut s = state();
        let order = |s: &State| s.cards().iter().map(Card::name).collect::<Vec<_>>();
        assert_eq!(order(&s), ["main", "a", "b"]);
        loaded(
            &mut s,
            vec![("/r", status(1)), ("/a", status(100)), ("/b", status(200))],
        );
        assert_eq!(order(&s), ["main", "a", "b"], "statuses never reorder");
    }

    #[test]
    fn stale_results_are_dropped_and_statuses_survive_a_refresh() {
        let mut s = state();
        loaded(&mut s, vec![("/a", status(5))]);
        let old = Loaded {
            request: s.request - 1,
            base: None,
            statuses: vec![("/b".into(), Ok(status(9)), None)],
        };
        let _ = s.update(Message::Loaded(Box::new(old)));
        assert!(
            s.cards()
                .iter()
                .all(|c| c.tree.path != Path::new("/b") || c.status.is_none())
        );
        s.set_worktrees(&[tree("/r", "main"), tree("/a", "a")]);
        assert_eq!(s.cards().len(), 2);
        assert!(
            s.cards().iter().any(|c| c.status.is_some()),
            "kept /a's status"
        );
    }

    #[test]
    fn a_click_shows_the_branch_and_a_second_click_opens_it() {
        let mut s = state();
        let u = s.update(Message::Click("/a".into()));
        assert!(matches!(u.output, Some(Output::ShowRevision(r)) if r == "refs/heads/a"));
        let u = s.update(Message::Click("/a".into()));
        assert!(matches!(u.output, Some(Output::OpenRepo(p)) if p.as_path() == Path::new("/a")));
    }

    #[test]
    fn a_click_after_a_keyboard_selection_does_not_open() {
        let mut s = state();
        let _ = s.update(Message::Select("/a".into()));
        let u = s.update(Message::Click("/a".into()));
        assert!(u.output.is_none(), "already selected, not a double click");
    }

    #[test]
    fn a_right_click_selects_shows_the_history_and_opens_the_menu() {
        let mut s = state();
        let _ = s.update(Message::Click("/a".into()));
        let u = s.update(Message::RightClick("/b".into()));
        assert!(matches!(
            u.output,
            Some(Output::Menu { show: Some(r), .. }) if r == "refs/heads/b"
        ));
        assert_eq!(s.selected().map(Card::name).as_deref(), Some("b"));
    }

    #[test]
    fn this_tabs_tree_cannot_be_removed_or_opened_again() {
        let mut s = state();
        let _ = s.update(Message::Select("/r".into()));
        assert!(s.output_for(Action::Remove).is_none());
        assert!(s.output_for(Action::Open).is_none());
        assert!(s.output_for(Action::Editor).is_some());
        // From a linked tree's tab, the main tree opens but stays.
        s.set_tabs(vec!["/r".into(), "/a".into()], "/a".into());
        assert!(s.output_for(Action::Open).is_some());
        assert!(s.output_for(Action::Remove).is_none());
        let _ = s.update(Message::Select("/a".into()));
        assert!(s.output_for(Action::Remove).is_none(), "the tab's own tree");
    }

    #[test]
    fn locked_and_missing_trees_are_not_removed() {
        let mut s = State::default();
        let mut locked = tree("/l", "l");
        locked.locked = true;
        let mut gone = tree("/g", "g");
        gone.prunable = true;
        s.set_worktrees(&[tree("/r", "main"), locked, gone]);
        s.set_tabs(vec![], "/r".into());
        let _ = s.update(Message::Select("/l".into()));
        assert!(s.output_for(Action::Remove).is_none());
        assert!(matches!(
            s.output_for(Action::Unlock),
            Some(Output::Lock(_, false))
        ));
        let _ = s.update(Message::Select("/g".into()));
        assert!(matches!(s.output_for(Action::Remove), Some(Output::Prune)));
    }

    #[test]
    fn removal_says_what_would_be_lost() {
        let mut s = state();
        let mut dirty = status(1);
        dirty.unstaged = 2;
        loaded(&mut s, vec![("/a", dirty)]);
        let _ = s.update(Message::Select("/a".into()));
        assert!(matches!(
            s.output_for(Action::Remove),
            Some(Output::Remove(Removal {
                changed: Some(2),
                detached: false,
                ..
            }))
        ));
    }

    #[test]
    fn compare_needs_work_beyond_the_base() {
        let mut s = state();
        let mut st = status(1);
        st.base = Some("main".into());
        st.merge_base = Some(oid('b'));
        loaded(&mut s, vec![("/a", st.clone())]);
        let _ = s.update(Message::Select("/a".into()));
        assert!(
            s.output_for(Action::Compare).is_none(),
            "no commits of its own"
        );
        st.ahead_of_base = 3;
        loaded(&mut s, vec![("/a", st)]);
        assert!(matches!(
            s.output_for(Action::Compare),
            Some(Output::Compare { count: 3, .. })
        ));
    }

    fn text(parts: Vec<(String, Part)>) -> String {
        parts.into_iter().map(|(s, _)| s).collect()
    }

    #[test]
    fn the_title_names_the_base_and_what_is_left_to_merge() {
        let mut s = state();
        let title = |s: &State, i: usize| text(title_parts(&s.cards()[i]));
        assert_eq!(title(&s, 1), "…", "still loading");
        let mut st = status(1);
        loaded(&mut s, vec![("/r", st.clone()), ("/a", st.clone())]);
        assert_eq!(title(&s, 1), "no base · b sets one");
        st.base = Some("main".into());
        loaded(&mut s, vec![("/a", st.clone())]);
        assert_eq!(title(&s, 1), "main ← up to date");
        st.ahead_of_base = 4;
        st.behind_base = 2;
        loaded(&mut s, vec![("/a", st.clone())]);
        let parts = title_parts(&s.cards()[1]);
        assert_eq!(text(parts.clone()), "main ← 4 to merge · 2 new");
        assert_eq!(parts[0].1, Part::Strong);
        // Narrow: whole pieces go from the end, the arrow with the last.
        assert_eq!(text(fit_parts(parts.clone(), 18)), "main ← 4 to merge");
        assert_eq!(text(fit_parts(parts, 6)), "main");
        st.base_guessed = true;
        loaded(&mut s, vec![("/a", st.clone())]);
        assert_eq!(
            title(&s, 1),
            "main ← 4 to merge · 2 new",
            "the guess shows only in the tooltip"
        );
        st.base_guessed = false;
        st.ahead_of_base = 0;
        st.had_work = true;
        loaded(&mut s, vec![("/a", st.clone())]);
        assert_eq!(title(&s, 1), "main ← all merged");
        st.last_commit = None;
        loaded(&mut s, vec![("/a", st)]);
        assert_eq!(title(&s, 1), "main ← no commits yet");
    }

    #[test]
    fn the_last_line_shows_the_upstream_in_arrows_and_cuts_the_subject_last() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let mut s = status(1000 - 15 * 60);
        s.last_commit.as_mut().expect("commit").subject = "Add refund flow".into();
        assert_eq!(text(remote_parts(&s, now)), "⊘ · Add refund flow 15m");
        s.upstream = Some("origin/feature/pay".into());
        s.ahead = 2;
        s.behind = 1;
        let parts = remote_parts(&s, now);
        assert_eq!(text(parts.clone()), "↑2 ↓1 · Add refund flow 15m");
        assert_eq!(
            sync_tip(&s),
            [
                "↑2 commits to push to origin/feature/pay",
                "↓1 commit to pull from origin/feature/pay"
            ]
        );
        assert_eq!(text(fit_parts(parts.clone(), 14)), "↑2 ↓1 · Add r…");
        // Too little room for a useful subject: it goes whole.
        assert_eq!(text(fit_parts(parts, 13)), "↑2 ↓1");
        s.ahead = 0;
        s.behind = 0;
        assert_eq!(
            text(remote_parts(&s, now)),
            "Add refund flow 15m",
            "in sync"
        );
    }

    #[test]
    fn the_changes_line_counts_in_words_and_flags_shared_files() {
        let mut s = state();
        let card = |s: &State| s.cards()[1].clone();
        assert_eq!(text(changes_parts(&card(&s))), "…");
        let mut st = status(1);
        loaded(&mut s, vec![("/a", st.clone())]);
        assert_eq!(text(changes_parts(&card(&s))), "clean");
        st.unstaged = 2;
        st.untracked = 1;
        st.conflicts = 1;
        loaded(&mut s, vec![("/a", st)]);
        let mut c = card(&s);
        assert_eq!(text(changes_parts(&c)), "1 conflict · 3 uncommitted");
        c.overlap = Some(Overlap {
            files: 2,
            tip: String::new(),
        });
        assert_eq!(
            text(changes_parts(&c)),
            "1 conflict · 3 uncommitted · ⚠ 2 shared"
        );
        c.tree.prunable = true;
        assert_eq!(text(changes_parts(&c)), "folder deleted");
    }

    #[test]
    fn one_word_says_what_a_tree_is_up_to_problems_first() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let mut s = state();
        let at = |s: &State| phase(&s.cards()[1], now);
        assert_eq!(at(&s), None, "still loading");
        let mut st = status(1);
        loaded(&mut s, vec![("/a", st.clone())]);
        assert_eq!(at(&s), Some(Phase::Clean));
        st.base = Some("main".into());
        st.behind_base = 3;
        loaded(&mut s, vec![("/a", st.clone())]);
        assert_eq!(at(&s), Some(Phase::Behind));
        st.ahead_of_base = 1;
        loaded(&mut s, vec![("/a", st.clone())]);
        assert_eq!(at(&s), Some(Phase::Ready));
        st.unstaged = 1;
        st.last_activity = Some(now - Duration::from_secs(600));
        loaded(&mut s, vec![("/a", st.clone())]);
        assert_eq!(at(&s), Some(Phase::Uncommitted));
        st.last_activity = Some(now - Duration::from_secs(30));
        loaded(&mut s, vec![("/a", st.clone())]);
        assert_eq!(at(&s), Some(Phase::Editing));
        st.conflicts = 1;
        loaded(&mut s, vec![("/a", st.clone())]);
        assert_eq!(at(&s), Some(Phase::Conflict));
        let mut merged = status(1);
        merged.base = Some("main".into());
        merged.had_work = true;
        loaded(&mut s, vec![("/a", merged)]);
        assert_eq!(at(&s), Some(Phase::Merged));
    }

    #[test]
    fn cleanup_lists_only_merged_clean_unlocked_trees() {
        let mut s = State::default();
        let mut locked = tree("/l", "l");
        locked.locked = true;
        s.set_worktrees(&[tree("/r", "main"), tree("/a", "a"), tree("/b", "b"), locked]);
        let mut merged = status(1);
        merged.base = Some("main".into());
        merged.behind_base = 2;
        merged.had_work = true;
        let mut dirty = merged.clone();
        dirty.unstaged = 1;
        loaded(
            &mut s,
            vec![("/a", merged.clone()), ("/b", dirty), ("/l", merged)],
        );
        let u = s.update(Message::Cleanup);
        assert!(matches!(u.output, Some(Output::Cleanup { base, trees })
            if base == "main" && trees == vec![(PathBuf::from("/a"), Some("a".to_owned()))]));
    }

    #[test]
    fn update_is_offered_only_when_the_base_moved_on() {
        let mut s = state();
        let mut behind = status(1);
        behind.base = Some("main".into());
        behind.behind_base = 2;
        let mut current = behind.clone();
        current.behind_base = 0;
        loaded(&mut s, vec![("/a", behind), ("/b", current)]);
        let _ = s.update(Message::Select("/a".into()));
        assert!(matches!(
            s.output_for(Action::Update),
            Some(Output::Update { ref branch, ref base, ref tree, behind: 2, .. })
                if branch == "a" && base == "main" && tree == Path::new("/a")
        ));
        let _ = s.update(Message::Select("/b".into()));
        assert!(
            s.output_for(Action::Update).is_none(),
            "nothing to bring in"
        );
        let _ = s.update(Message::Select("/r".into()));
        assert!(s.output_for(Action::Update).is_none(), "no status, no base");
    }

    #[test]
    fn shared_files_match_exactly_or_under_an_untracked_folder() {
        let a = paths(&["new/", "src/a.rs", "src/b.rs", "z.rs"]);
        let b = paths(&["new/x.rs", "other/", "src/a.rs", "src/c.rs"]);
        let c = paths(&["other/deep/y.rs", "z.rs"]);
        assert_eq!(
            shared_files(&a, &b),
            [Path::new("new/"), Path::new("src/a.rs")]
        );
        assert_eq!(
            shared_files(&b, &a),
            [Path::new("new/x.rs"), Path::new("src/a.rs")]
        );
        assert_eq!(shared_files(&c, &b), [Path::new("other/deep/y.rs")]);
        assert!(shared_files(&paths(&["src"]), &paths(&["src-x/a"])).is_empty());
    }

    #[test]
    fn overlaps_count_files_once_and_name_each_other_tree() {
        let a = paths(&["f1", "f2", "f3", "f4", "f5"]);
        let b = paths(&["f1", "f2", "f3", "f4"]);
        let c = paths(&["f1", "x"]);
        let d = paths(&["y"]);
        let names = ["a", "b", "c", "d", "e"].map(String::from);
        let found = overlaps(&[Some(&a), Some(&b), Some(&c), Some(&d), None], &names);
        let a = found[0].as_ref().expect("a overlaps");
        assert_eq!(a.files, 4);
        assert_eq!(
            a.tip,
            "other worktrees change these files too\n\
             b: f1, f2, f3 (+1)\n\
             c: f1\n\
             merging both may conflict"
        );
        assert_eq!(found[2].as_ref().map(|o| o.files), Some(1));
        assert!(found[3].is_none(), "nothing shared");
        assert!(found[4].is_none(), "unknown: not compared");
    }

    #[test]
    fn the_tabs_own_tree_is_selected_until_another_is_picked() {
        let mut s = state();
        assert_eq!(s.selected().map(Card::name).as_deref(), Some("main"));
        let _ = s.update(Message::Select("/a".into()));
        s.set_worktrees(&[tree("/r", "main"), tree("/b", "b"), tree("/a", "a")]);
        assert_eq!(s.selected().map(Card::name).as_deref(), Some("a"), "kept");
        // Opening the tab again (from another tree) marks its own tree.
        s.set_tabs(vec!["/r".into(), "/b".into()], "/b".into());
        s.select_here();
        assert_eq!(s.selected().map(Card::name).as_deref(), Some("b"));
    }

    #[test]
    fn tabs_match_by_the_resolved_folder() {
        let mut s = state();
        s.set_tabs(vec!["/real/a".into()], "/r".into());
        assert!(!s.cards().iter().any(|c| c.in_tab));
        s.request += 1;
        let request = s.request;
        let _ = s.update(Message::Loaded(Box::new(Loaded {
            request,
            base: None,
            statuses: vec![("/a".into(), Ok(status(1)), Some("/real/a".into()))],
        })));
        assert!(s.cards().iter().any(|c| c.in_tab && c.name() == "a"));
    }

    #[test]
    fn keys_move_and_act_on_the_selection() {
        let t = crate::theme::preset("gf-dark");
        let mut s = state();
        s.selected = None;
        let m = s.nav(Nav::plain(Key::Down), &t).expect("moves");
        assert!(matches!(m.message, Some(Message::Select(p)) if p.as_path() == Path::new("/r")));
        let m = s.nav(Nav::plain(Key::Char('l')), &t).expect("used");
        assert!(m.message.is_none(), "nothing selected yet");
        let _ = s.update(Message::Select("/a".into()));
        let m = s.nav(Nav::plain(Key::Char('d')), &t).expect("used");
        assert!(matches!(m.message, Some(Message::Act(Action::Remove))));
        let m = s.nav(Nav::plain(Key::Char('l')), &t).expect("used");
        assert!(matches!(m.message, Some(Message::Act(Action::Lock))));
        assert!(
            s.nav(Nav::plain(Key::Left), &t).is_none(),
            "left moves panes"
        );
    }

    #[test]
    fn merges_into_the_base_in_the_tree_that_has_it() {
        let mut s = State::default();
        s.set_worktrees(&[tree("/r", "develop"), tree("/a", "a"), tree("/b", "b")]);
        s.set_tabs(vec!["/r".into()], "/r".into());
        let mut work = status(1);
        work.base = Some("develop".into());
        work.ahead_of_base = 3;
        let mut elsewhere = work.clone();
        elsewhere.base = Some("release".into());
        loaded(&mut s, vec![("/a", work), ("/b", elsewhere)]);

        let _ = s.update(Message::Select("/a".into()));
        assert!(matches!(
            s.output_for(Action::Merge),
            Some(Output::Merge { ref branch, ref base, ref into, count: 3, .. })
                if branch == "a" && base == "develop" && into == Path::new("/r")
        ));
        assert!(matches!(
            s.output_for(Action::SetBase),
            Some(Output::SetBase { ref base, guessed: false, .. }) if base.as_deref() == Some("develop")
        ));
        // No tree has `release` checked out: nowhere to merge.
        let _ = s.update(Message::Select("/b".into()));
        assert!(s.output_for(Action::Merge).is_none());
        assert!(s.output_for(Action::SetBase).is_some());
    }

    #[test]
    fn menu_offers_only_what_applies() {
        let mut s = state();
        let _ = s.update(Message::Select("/a".into()));
        let menu = s.menu().expect("menu");
        let labels: Vec<_> = menu
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item { label, .. } => Some(label.as_str()),
                Entry::Separator => None,
            })
            .collect();
        assert!(labels.contains(&"open in tab") && labels.contains(&"remove…"));
        assert!(!labels.contains(&"unlock") && !labels.contains(&"compare with base"));
    }
}
