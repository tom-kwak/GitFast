//! Worktrees view: the second mode of the refs panel (`REFS │ WORKTREES`,
//! ⌘⇧B). One card per working tree of the repository, for watching and
//! steering several trees at once (e.g. one per AI agent):
//!
//! ```text
//! ● main                              ✓
//!   base branch
//! ◉ feature/payment                ⚡ ●3
//!   from main · 4 ahead · 2 behind    ↑2
//! ```
//!
//! - Line 1: branch, files touched lately (`⚡`), lock, and what is
//!   uncommitted there (`●N`, `✕N` conflicts). In front, `●` marks the tree
//!   this tab shows and `◉` one open in another tab. That tree is also
//!   the selected card until another is picked, and again whenever the tab
//!   is opened from another tree.
//! - Line 2: in words, the branch it came from and how it stands against
//!   it: commits it has that the base lacks (`ahead`), commits the base got
//!   since (`behind`), or `merged` once the base has its work; then the
//!   upstream distance (`↑↓`). Hovering it shows the last commit.
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
use crate::widgets::{self, colored, dim, format};

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

/// Heights of a card's lines: the name at the base size, the two detail
/// lines at the small size. Fixed, so badges never move the text.
fn line_heights(t: &Tokens) -> (f32, f32) {
    ((t.font_size * 1.35).ceil(), (t.small() * 1.35).ceil())
}

/// A card: two lines, its padding and the gap to the next one.
pub fn row_height(t: &Tokens) -> f32 {
    let (big, small) = line_heights(t);
    big + small + 2.0 * PAD_Y + 2.0 + GAP
}

/// Width of the mark column in front of the name (`●`).
fn gutter(t: &Tokens) -> f32 {
    t.char_width() * 1.5
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

        // Line 1: name and what is uncommitted.
        let status: Element<'a, Message> = if c.tree.prunable {
            widgets::tip(
                t,
                badge("missing".into(), t.error()),
                "folder deleted · prune cleans it up".into(),
            )
        } else if let Some(e) = &c.error {
            widgets::tip(t, badge("?".into(), t.error()), format::truncate(e, 200))
        } else if let Some(s) = &c.status {
            let changed = s.changed() - s.conflicts;
            let mut parts = Vec::new();
            if s.conflicts > 0 {
                parts.push((format!("✕{}", s.conflicts), t.error()));
            }
            if changed > 0 {
                parts.push((format!("●{changed}"), t.warning()));
            }
            if parts.is_empty() {
                widgets::tip(
                    t,
                    badge("✓".into(), t.success()),
                    "nothing to commit".into(),
                )
            } else {
                widgets::tip(
                    t,
                    Row::with_children(parts.into_iter().map(|(s, color)| badge(s, color).into()))
                        .spacing(4),
                    format!(
                        "{} staged · {} unstaged · {} untracked · {} conflicted",
                        s.staged, s.unstaged, s.untracked, s.conflicts
                    ),
                )
            }
        } else {
            dim(t, "…").into()
        };
        // Line 1: files touched lately, lock, then the status.
        let mut tags: Vec<(Element<'a, Message>, usize)> = Vec::new();
        if let Some(ago) = c
            .status
            .as_ref()
            .and_then(|s| s.last_activity)
            .and_then(|at| now.duration_since(at).ok())
            .filter(|ago| *ago < ACTIVE_FOR)
        {
            tags.push((
                widgets::tip(
                    t,
                    badge("⚡".into(), t.warning()),
                    format!("files changed {} ago", format::age(ago)),
                ),
                2,
            ));
        }
        if let Some(o) = &c.overlap {
            let text = format!("⚠{}", o.files);
            let cols = crate::widgets::virtual_list::columns(&text);
            tags.push((
                widgets::tip(t, badge(text, t.warning()), o.tip.clone()),
                cols,
            ));
        }
        if c.tree.locked {
            tags.push((
                widgets::tip(
                    t,
                    badge("locked".into(), t.warning()),
                    "can't be removed until unlocked".into(),
                ),
                6,
            ));
        }
        let used: usize = tags.iter().map(|(_, n)| n + 1).sum::<usize>() + status_columns(c);
        let name_cols = scale_columns(cols.saturating_sub(used + 1), t.small() / t.font_size);
        let name_color = if c.current { t.head() } else { t.branch() };
        let mark: Element<'a, Message> = if c.current {
            widgets::tip(t, colored(t, "●", t.head()), "open in this tab".into())
        } else if c.in_tab {
            widgets::tip(t, colored(t, "◉", t.accent()), "open in another tab".into())
        } else {
            space::horizontal().into()
        };
        let mut badges: Vec<Element<'a, Message>> = tags.into_iter().map(|(e, _)| e).collect();
        badges.push(status);
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
            badges,
        );

        // Line 2: where it comes from and how it stands against that branch
        // (`from main · 4 ahead · 2 behind`), then the upstream distance.
        let mut marks: Vec<(Element<'a, Message>, usize)> = Vec::new();
        let (parts, tip) = match &c.status {
            Some(s) => {
                let sync = match (s.ahead, s.behind) {
                    (0, 0) => None,
                    (a, 0) => Some(format!("↑{a}")),
                    (0, b) => Some(format!("↓{b}")),
                    (a, b) => Some(format!("↑{a}↓{b}")),
                };
                if let Some(sync) = sync {
                    let n = columns(&sync);
                    marks.push((
                        widgets::tip(
                            t,
                            badge(sync, t.remote()),
                            format!("vs {}", s.upstream.as_deref().unwrap_or("upstream")),
                        ),
                        n,
                    ));
                }
                let room = cols.saturating_sub(marks.iter().map(|(_, n)| n + 1).sum::<usize>());
                let on_base = self.base.is_some() && c.tree.branch == self.base;
                let all = base_parts(s, on_base);
                let full: String = all.iter().map(|(p, _)| p.as_str()).collect();
                let parts = fit_parts(all, room);
                let mut tip = Vec::new();
                // A line cut to fit shows whole in the tooltip.
                if parts.iter().map(|(p, _)| p.as_str()).collect::<String>() != full {
                    tip.push(full);
                }
                if let Some(base) = &s.base {
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
                }
                if let Some(lc) = &s.last_commit {
                    let age = u64::try_from(lc.time)
                        .ok()
                        .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                        .and_then(|at| now.duration_since(at).ok())
                        .map_or_else(String::new, |d| format!(" · {}", format::age(d)));
                    tip.push(format!(
                        "last commit {} {}{age}",
                        lc.id.short(),
                        format::truncate(&lc.subject, 60)
                    ));
                }
                (parts, tip.join("\n"))
            }
            None => (Vec::new(), String::new()),
        };
        let text: Element<'a, Message> = Row::with_children(parts.into_iter().map(|(s, part)| {
            let color = match part {
                Part::Plain => t.dim,
                Part::Base => t.branch(),
                Part::Ahead => t.added(),
                Part::Behind => t.warning(),
                Part::Merged => t.success(),
            };
            colored(t, s, color)
                .size(t.small())
                .wrapping(Wrapping::None)
                .into()
        }))
        .into();
        let second = line(
            t,
            small,
            space::horizontal().into(),
            if tip.is_empty() {
                text
            } else {
                widgets::tip(t, text, tip)
            },
            marks.into_iter().map(|(e, _)| e).collect(),
        );

        let body = button(column![first, second])
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
}

/// What a piece of a card's second line says, for its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Plain,
    Base,
    Ahead,
    Behind,
    Merged,
}

/// A tree against its base, in words: `from main · 4 ahead · 2 behind`,
/// `from main · merged`, `base branch` for the tree on the base itself.
fn base_parts(s: &WorktreeStatus, on_base: bool) -> Vec<(String, Part)> {
    if s.last_commit.is_none() {
        return vec![("no commits yet".into(), Part::Plain)];
    }
    let Some(base) = &s.base else {
        return if on_base {
            vec![("base branch".into(), Part::Plain)]
        } else {
            Vec::new()
        };
    };
    let mut parts = vec![("from ".into(), Part::Plain), (base.clone(), Part::Base)];
    if s.merged() {
        parts.push((" · ".into(), Part::Plain));
        parts.push(("merged".into(), Part::Merged));
        return parts;
    }
    if s.ahead_of_base > 0 {
        parts.push((" · ".into(), Part::Plain));
        parts.push((format!("{} ahead", s.ahead_of_base), Part::Ahead));
    }
    if s.behind_base > 0 {
        parts.push((" · ".into(), Part::Plain));
        parts.push((format!("{} behind", s.behind_base), Part::Behind));
    }
    parts
}

/// `parts` cut to `room` columns: whole `· …` pieces drop from the end, and
/// only the base name itself is shortened with `…`.
fn fit_parts(mut parts: Vec<(String, Part)>, room: usize) -> Vec<(String, Part)> {
    let width = |parts: &[(String, Part)]| parts.iter().map(|(s, _)| columns(s)).sum::<usize>();
    // Drop the trailing `· x` pairs first, keeping `from <base>`.
    while parts.len() > 2 && width(&parts) > room {
        parts.truncate(parts.len() - 2);
    }
    let used = width(&parts);
    if used > room
        && let Some((last, _)) = parts.last_mut()
    {
        let keep = columns(last).saturating_sub(used - room);
        *last = format::truncate_columns(last, keep);
    }
    parts
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

/// Columns of a card's status badge (line 1), for cutting the name.
fn status_columns(c: &Card) -> usize {
    if c.tree.prunable {
        return 7;
    }
    match (&c.error, &c.status) {
        (Some(_), _) | (None, None) => 1,
        (None, Some(s)) => {
            let changed = s.changed() - s.conflicts;
            let mut n = 0;
            if s.conflicts > 0 {
                n += 1 + s.conflicts.to_string().len();
            }
            if changed > 0 {
                n += 1 + changed.to_string().len() + usize::from(n > 0);
            }
            n.max(1)
        }
    }
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

    #[test]
    fn the_second_line_says_where_a_tree_comes_from_in_words() {
        let text =
            |parts: Vec<(String, Part)>| parts.into_iter().map(|(s, _)| s).collect::<String>();
        let mut s = status(1);
        assert_eq!(text(base_parts(&s, true)), "base branch");
        assert_eq!(text(base_parts(&s, false)), "");
        s.base = Some("main".into());
        assert_eq!(text(base_parts(&s, false)), "from main");
        s.ahead_of_base = 4;
        s.behind_base = 2;
        let parts = base_parts(&s, false);
        assert_eq!(text(parts.clone()), "from main · 4 ahead · 2 behind");
        assert_eq!(parts[1].1, Part::Base);
        assert_eq!(parts[3].1, Part::Ahead);
        // Narrow: whole pieces go from the end, then the name is cut.
        assert_eq!(text(fit_parts(parts.clone(), 20)), "from main · 4 ahead");
        assert_eq!(text(fit_parts(parts, 7)), "from m…");
        s.ahead_of_base = 0;
        s.had_work = true;
        assert_eq!(text(base_parts(&s, false)), "from main · merged");
        s.last_commit = None;
        assert_eq!(text(base_parts(&s, false)), "no commits yet");
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
