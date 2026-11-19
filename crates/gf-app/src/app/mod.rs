//! Root of the UI: owns feature states, routes messages, reacts to feature
//! outputs and lays out the window.
//!
//! ```text
//! ┌ tabs ─────────────────────────────────────────────────────────┐
//! ├ toolbar: fetch pull push │ branch stash refresh ──────────────┤
//! │ refs     │ history (graph)                  │ working tree     │
//! │ sidebar  ├──────────────────────────────────┤                  │
//! │          │ diff                             ├──────────────────┤
//! │          │                                  │ commit           │
//! ├ command log (⌘J) ─────────────────────────────────────────────┤
//! │ gf ❯ prompt                                                   │
//! └ status line ──────────────────────────────────────────────────┘
//! ```
//!
//! Split by concern: `ops` runs git work, `outputs` reacts to feature
//! outputs, `dialogs` owns confirmation/input dialogs.

mod advanced;
mod branch_delete;
mod dialogs;
mod focus;
mod ops;
mod outputs;
mod reveal;
mod settings;
#[cfg(test)]
mod ui_sweep;
mod undoing;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gf_config::{AppPaths, Settings};
use gf_core::model::RepoId;
use gf_core::{CoreEvent, EventBus};
use gf_git::{CancellationToken, GitRunner, Progress, Repository};
use iced::futures::StreamExt;
use iced::widget::{center, column, container, opaque, operation, row, stack};
use iced::{
    Background, Color, Element, Length, Padding, Subscription, Task, event, keyboard, window,
};

use self::dialogs::Purpose;
use crate::feature::{Cx, Feature, Update};
use crate::features::code_view;
use crate::features::command_log::NoteKind;
use crate::features::context_menu;
use crate::features::dialog::{self, Dialog};
use crate::features::prompt::PROMPT_ID;
use crate::features::status_line::Flash;
use crate::features::{
    changes, command_log, commit_box, diff_view, history, prompt, repo_picker, settings_view,
    sidebar, status_line, tab_list, tab_switcher, tabs, toolbar, welcome,
};
use crate::nav::{Key, Nav, Pane};
use crate::repo_tab::{History, RepoTab, Snapshot};
use crate::theme::{self, Tokens, style};
use crate::widgets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Open,
    /// ⌘,: app-wide preferences.
    Settings,
    /// ⌘⌫: forget the selected repository in the open window.
    Forget,
    /// ⌃⇥ / ⌃⇧⇥: walk the tabs in most-recently-used order.
    CycleTab {
        back: bool,
    },
    /// ⌘P: the dropdown of all tabs (`▾ N tabs`).
    TabList,
    /// ⌘Z / ⌘⇧Z outside text fields.
    Undo,
    Redo,
    /// ⌘⇧[ / ⌘⇧]: the tab to the left / right.
    PrevTab,
    NextTab,
    CloseTab,
    Refresh,
    SelectTab(usize),
    FocusPrompt,
    ToggleLog,
    ToggleRefs,
    ToggleHistory,
    ToggleLocal,
    HistoryPrev,
    HistoryNext,
    Complete,
    Commit,
    Escape,
    Enter,
    /// ⌘[ / ⌘]: HISTORY / WORKING TREE mode of the main area.
    CodeView(diff_view::Role),
    FindInFile,
    FindMatch {
        back: bool,
    },
    /// ⌘C outside text fields: the selected diff lines.
    Copy,
    /// ⌘A outside text fields: every line of the diff in focus.
    SelectAll,
    Quit,
}

/// What to do after an operation finished, besides refreshing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpKind {
    Generic,
    Checkout(PathBuf),
    DeleteBranches(PathBuf),
    RemoveWorktree(PathBuf),
    Commit,
    Fetch,
    Push {
        branch: Option<String>,
        /// Explicit destination; `None` pushes the current branch.
        to: Option<PushTo>,
    },
    Pull,
    /// Undoing (or redoing) a step: it moves to the other stack once this
    /// succeeds.
    Undo {
        redo: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum CheckoutPhase {
    Measuring(RepoId),
    Running,
    RefreshQueued,
    Refreshing(RepoId),
}

#[derive(Debug)]
struct Checkout {
    /// Full ref of the row the user activated, or a revision for a detached checkout.
    target: String,
    phase: CheckoutPhase,
}

/// Where to push a branch that is not the current one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTo {
    pub remote: String,
    /// `<local branch>:<branch on the remote>`.
    pub refspec: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tabs(tabs::Message),
    Toolbar(toolbar::Message),
    Welcome(welcome::Message),
    Prompt(prompt::Message),
    Log(command_log::Message),
    Sidebar(sidebar::Message),
    Changes(changes::Message),
    History(history::Message),
    LocalDiff(diff_view::Message),
    CommitDiff(diff_view::Message),
    /// Switch the code view between local and commit changes.
    CodeView(diff_view::Role),
    Commit(commit_box::Message),
    Dialog(dialog::Message),
    CodeDialog(code_view::Message),
    PathRevealed(Result<(), String>),
    Picker(repo_picker::Message),
    Switcher(tab_switcher::Message),
    TabList(tab_list::Message),
    Settings(settings_view::Message),
    /// Show the open-repository window (⌘O, `+ open`).
    OpenPicker,
    /// An animation frame (only subscribed to while something animates).
    Frame,

    Core(CoreEvent),
    Key(Shortcut),
    /// A key press from the keyboard (see `focus` for who gets it).
    KeyInput(focus::KeyInput),
    /// A panel was clicked: it takes the arrow keys.
    FocusPane(Pane),
    Menu(context_menu::Message),
    Tick,
    FileHovered,
    FileHoverLeft,
    FileDropped(PathBuf),
    /// ⌘ (Ctrl elsewhere) pressed or released.
    /// Keyboard modifiers changed (shift/⌘ change what clicks do).
    Modifiers(keyboard::Modifiers),

    Open(PathBuf),
    Opened(PathBuf, Result<Box<Repository>, String>),
    Restored(Vec<Repository>, Option<usize>),
    PickFolder,
    Picked(Option<PathBuf>),
    Loaded(RepoId, Result<Arc<Snapshot>, String>),
    HistoryLoaded(RepoId, u64, String, Result<Arc<History>, String>),
    DiffFinished(RepoId, diff_view::Role, diff_view::Completion),
    AmendTextLoaded(RepoId, Option<String>),
    SyncProgress(Progress),
    OpDone {
        repo: RepoId,
        label: String,
        kind: OpKind,
        result: Result<(), String>,
        /// What it changed, to undo it.
        step: Option<Box<crate::undo::Step>>,
    },
    IntegrateDone {
        repo: RepoId,
        label: String,
        result: Result<gf_git::Integration, String>,
        record: Option<Box<advanced::Recorded>>,
    },
    /// The repository state an undo / redo is about to replace.
    UndoChecked {
        repo: RepoId,
        redo: bool,
        now: Result<Box<gf_git::Capture>, String>,
    },
    Banner(crate::features::op_banner::Message),
    Conflict(crate::features::conflict_view::Message),
    Blame(crate::features::blame_view::Message),
    BlameLoaded(RepoId, u64, PathBuf, Result<Vec<gf_git::BlameLine>, String>),
    BlameHighlighted(
        RepoId,
        u64,
        PathBuf,
        Option<Arc<Vec<crate::highlight::Spans>>>,
    ),
    ConflictLoaded(
        RepoId,
        PathBuf,
        Result<Option<Vec<gf_git::conflict::Segment>>, String>,
    ),
    BisectDone(RepoId, Result<gf_git::BisectProgress, String>),
    /// Plan for the rebase editor; the ids, when given, are preset to squash.
    RebasePlanLoaded(
        RepoId,
        gf_core::model::Oid,
        Result<Vec<gf_git::TodoItem>, String>,
        Option<Vec<gf_core::model::Oid>>,
    ),
    RebaseEditor(crate::features::rebase_editor::Message),
    /// What the squash dialog shows; `selected` is how many of the ids the
    /// user picked (the rest filled a gap).
    SquashPreviewed {
        repo: RepoId,
        ids: Vec<gf_core::model::Oid>,
        selected: usize,
        result: Result<gf_git::SquashPreview, String>,
    },
    /// Undo the latest step of the active tab (⌘Z).
    Undo,
    /// Redo the latest undone step (⌘⇧Z).
    Redo,
    /// The undo history list (status line).
    UndoList(crate::features::undo_list::Message),
    /// The discard confirmation can be shown: `fits` says whether a copy
    /// of what it throws away can be kept for undo.
    DiscardMeasured {
        repo: RepoId,
        tracked: Vec<std::path::PathBuf>,
        untracked: Vec<std::path::PathBuf>,
        fits: bool,
    },
    /// What would keep the changes from coming along to branch `name`:
    /// the switch dialog can be shown.
    SwitchMeasured {
        repo: RepoId,
        name: String,
        remote: bool,
        /// Branch the changes are on.
        from: String,
        changed: usize,
        blocked: Result<Vec<PathBuf>, String>,
    },
    /// Where a commit to find is in the main history (see `reveal`).
    CommitLocated {
        repo: RepoId,
        oid: gf_core::model::Oid,
        result: Result<Option<reveal::Located>, String>,
    },
    /// A drag on the file list / diff divider starts.
    ResizeListStart,
    /// The divider is being dragged: pixels from where the drag started.
    ResizeList(f32),
    /// The divider was let go: remember the width.
    ResizeListDone,
    /// Double-click on the divider: back to the default width.
    ResizeListReset,
    GitFinished {
        repo: Option<RepoId>,
        args: Vec<String>,
        result: Result<(), String>,
        open_after: Option<PathBuf>,
    },
}

#[derive(Debug)]
pub struct App {
    paths: AppPaths,
    settings: Settings,
    tokens: Tokens,
    /// List width when the divider drag started (see `ResizeList`).
    list_drag_start: Option<f32>,
    bus: EventBus,
    runner: GitRunner,
    tabs: Vec<RepoTab>,
    active: usize,
    prompt: prompt::State,
    log: command_log::State,
    dialog: Option<Dialog<Purpose>>,
    code_dialog: Option<code_view::State>,
    code_dialog_request: u64,
    settings_open: bool,
    /// Open-repository window (modal, under a confirmation it asks for).
    picker: Option<repo_picker::State>,
    /// Tabs by last use, for ⌃⇥.
    mru: tab_switcher::Mru,
    /// ⌃⇥ switcher shown: the highlighted position in the MRU order.
    switcher: Option<usize>,
    /// The `▾ N tabs` dropdown, when open.
    tab_list: Option<tab_list::State>,
    /// The undo history list, when open.
    undo_list: Option<crate::features::undo_list::State>,
    /// Open action menu (right-click / `actions ▾`).
    menu: Option<context_menu::State<Message>>,
    /// Where the mouse was last pressed: menus open there.
    pointer: crate::widgets::pointer::Pointer,
    /// Interactive rebase plan being edited (modal).
    /// Running fetch/pull/push and its cancel handle.
    sync: Option<(toolbar::Sync, CancellationToken)>,
    /// Keyed by worktree git directory so a running switch survives closing its tab.
    checkouts: HashMap<PathBuf, Checkout>,
    deleting_branches: HashSet<PathBuf>,
    flash: Option<Flash>,
    tick: usize,
    drop_hover: bool,
    /// While ⌘ is held, characters a text field inserts are shortcut keys,
    /// not typing (see `Message::Prompt`).
    /// Currently held modifiers.
    modifiers: keyboard::Modifiers,
    /// Repositories being opened (for the busy indicator).
    opening: usize,
    /// Panel the arrow keys drive while no text field has focus.
    focus: Pane,
    /// The user is typing in the prompt: give it focus back after a
    /// dialog, menu or command.
    typing: bool,
    /// The menu being opened was asked for from the keyboard.
    menu_by_key: bool,
}

impl App {
    pub fn boot(paths: AppPaths, settings: Settings) -> (Self, Task<Message>) {
        let bus = EventBus::default();
        let mut runner = GitRunner::new()
            .with_event_bus(bus.clone())
            // gf has no interactive editor yet: make git fail fast instead of
            // hanging on an invisible vi (see git_actions::hint_for_error).
            .with_env([("GIT_EDITOR", "false"), ("GIT_SEQUENCE_EDITOR", "false")]);
        if let Some(git) = &settings.git.executable {
            runner = runner.with_git_path(git);
        }
        let tokens =
            theme::preset(&settings.appearance.theme).with_font_size(settings.appearance.font_size);

        // Restore tabs from last session, in their original order.
        let to_restore = settings.repos.open.clone();
        let no_tabs = to_restore.is_empty();
        let active = settings.repos.active;
        let restore = if to_restore.is_empty() {
            Task::none()
        } else {
            let runner = runner.clone();
            Task::perform(
                async move {
                    // Opened all at once (each asks git where the repository
                    // is); the tabs keep their saved order.
                    let opened = iced::futures::future::join_all(
                        to_restore
                            .iter()
                            .map(|path| Repository::open(path, runner.clone())),
                    )
                    .await;
                    let mut repos = Vec::new();
                    for (path, result) in to_restore.iter().zip(opened) {
                        match result {
                            Ok(r) => repos.push(r),
                            Err(e) => {
                                tracing::warn!(path = %path.display(), error = %e, "could not restore tab")
                            }
                        }
                    }
                    repos
                },
                move |repos| Message::Restored(repos, active),
            )
        };

        let app = Self {
            paths,
            settings,
            tokens,
            list_drag_start: None,
            bus,
            runner,
            tabs: Vec::new(),
            active: 0,
            prompt: prompt::State::default(),
            log: command_log::State::default(),
            dialog: None,
            code_dialog: None,
            code_dialog_request: 0,
            settings_open: false,
            picker: None,
            mru: tab_switcher::Mru::default(),
            switcher: None,
            tab_list: None,
            undo_list: None,
            menu: None,
            pointer: crate::widgets::pointer::Pointer::default(),
            sync: None,
            checkouts: HashMap::new(),
            deleting_branches: HashSet::new(),
            flash: None,
            tick: 0,
            drop_hover: false,
            modifiers: keyboard::Modifiers::empty(),
            opening: 0,
            focus: Pane::default(),
            typing: no_tabs,
            menu_by_key: false,
        };
        let focus = if no_tabs {
            operation::focus(PROMPT_ID)
        } else {
            Task::none()
        };
        (app, Task::batch([restore, focus]))
    }

    pub fn title(&self) -> String {
        match self.active_tab() {
            Some(tab) => match tab.head_label() {
                Some(head) => format!("gf — {} ⎇ {head}", tab.name()),
                None => format!("gf — {}", tab.name()),
            },
            None => "gf".into(),
        }
    }

    pub fn theme(&self) -> iced::Theme {
        self.tokens.iced_theme()
    }

    fn rebase_editor(&self) -> Option<&crate::features::rebase_editor::State> {
        self.active_tab()?.rebase_editor.as_ref()
    }

    fn active_tab(&self) -> Option<&RepoTab> {
        self.tabs.get(self.active)
    }

    fn active_tab_mut(&mut self) -> Option<&mut RepoTab> {
        self.tabs.get_mut(self.active)
    }

    fn tab_mut(&mut self, id: RepoId) -> Option<&mut RepoTab> {
        self.tabs.iter_mut().find(|t| t.repo.id() == id)
    }

    fn busy(&self) -> bool {
        self.opening > 0
            || self.sync.is_some()
            || !self.checkouts.is_empty()
            || !self.deleting_branches.is_empty()
            || self.tabs.iter().any(|t| t.loading)
            || self.log.has_running()
    }

    fn flash(&mut self, kind: NoteKind, text: impl Into<String>) {
        self.flash = Some(Flash {
            kind,
            text: text.into(),
        });
    }

    fn save_settings(&mut self) {
        self.settings.repos.open = self
            .tabs
            .iter()
            .map(|t| t.repo.cwd().to_path_buf())
            .collect();
        self.settings.repos.active = (!self.tabs.is_empty()).then_some(self.active);
        if let Err(e) = self.settings.save(&self.paths.settings_file()) {
            self.log.note(
                NoteKind::Error,
                "could not save settings",
                vec![e.to_string()],
            );
        }
    }

    /// Make the commit list visible when an action explicitly shows history.
    fn expand_history(&mut self) {
        if !self.settings.appearance.show_history {
            self.settings.appearance.show_history = true;
            self.save_settings();
        }
    }

    // ------------------------------------------------------------- update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        // Any message may have brought a tab to the front or queued a read.
        let task = Task::batch([task, self.load_due()]);
        // Anything may have written to the command log (notes, git output).
        match self.log.follow() {
            follow if follow.units() == 0 => task,
            follow => Task::batch([task, follow.map(Message::Log)]),
        }
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tabs(m) => match m {
                tabs::Message::Select(i) => self.select_tab(i),
                tabs::Message::Close(i) => self.close_tab(i),
                tabs::Message::Open => self.update(Message::OpenPicker),
                tabs::Message::Settings => self.toggle_settings(),
                tabs::Message::List => self.toggle_tab_list(),
            },
            Message::Toolbar(m) => self.on_toolbar(m),
            Message::Welcome(m) => match m {
                welcome::Message::OpenRecent(p) => self.update(Message::Open(p)),
                welcome::Message::Open => self.update(Message::OpenPicker),
            },
            Message::Modifiers(m) => {
                self.modifiers = m;
                // Letting go of ⌃ picks the highlighted tab.
                if !m.control() && self.switcher.is_some() {
                    return self.finish_switch();
                }
                Task::none()
            }
            Message::TabList(m) => self.on_tab_list(m),
            Message::Settings(m) => self.on_settings(m),
            Message::CodeDialog(m) => self.on_code_view(m),
            Message::PathRevealed(result) => {
                if let Err(e) = result {
                    self.flash(NoteKind::Error, format!("could not show it: {e}"));
                }
                Task::none()
            }
            Message::Switcher(tab_switcher::Message::Pick(i)) => {
                self.switcher = None;
                self.select_tab(i)
            }
            // Keys belong to the open menu or settings.
            Message::Prompt(_)
                if self.menu.is_some()
                    || self.settings_open
                    || self.code_dialog.is_some()
                    || self.dialog.is_some() =>
            {
                Task::none()
            }
            // ⌘O must open a folder, not type "o" into the prompt.
            Message::Prompt(prompt::Message::Input(_)) if self.modifiers.command() => Task::none(),
            Message::Prompt(m) => {
                if matches!(m, prompt::Message::Input(_) | prompt::Message::Submit) {
                    self.typing = true;
                }
                let Update { task, output } = self.prompt.update(m);
                let task = task.map(Message::Prompt);
                match output {
                    Some(prompt::Output::Run {
                        cmd,
                        converted_from,
                    }) => {
                        if let Some(original) = converted_from {
                            self.log.note(
                                NoteKind::Info,
                                format!("'{original}' was typed with the Korean input method"),
                                vec!["converted to the English keys you pressed".into()],
                            );
                        }
                        Task::batch([task, self.run_prompt_command(cmd)])
                    }
                    Some(prompt::Output::Invalid { line, error }) => {
                        self.log.note(NoteKind::Error, line, vec![error.clone()]);
                        self.flash(NoteKind::Error, error);
                        task
                    }
                    None => task,
                }
            }
            Message::Log(m) => {
                let Update { task, output } = self.log.update(m);
                match output {
                    Some(command_log::Output::Collapse) => self.on_shortcut(Shortcut::ToggleLog),
                    None => task.map(Message::Log),
                }
            }
            Message::Sidebar(m) => {
                let m = match m {
                    sidebar::Message::Select(sidebar::Selection::Ref(name))
                        if (name.starts_with("refs/heads/")
                            || name.starts_with("refs/remotes/"))
                            && self.modifiers.shift() =>
                    {
                        sidebar::Message::SelectBranchRange(name)
                    }
                    sidebar::Message::Select(sidebar::Selection::Ref(name))
                        if (name.starts_with("refs/heads/")
                            || name.starts_with("refs/remotes/"))
                            && self.modifiers.command() =>
                    {
                        sidebar::Message::ToggleBranch(name)
                    }
                    other => other,
                };
                if matches!(
                    m,
                    sidebar::Message::Select(_)
                        | sidebar::Message::RightClick(_)
                        | sidebar::Message::Toggle(_)
                        | sidebar::Message::ToggleBranch(_)
                        | sidebar::Message::SelectBranchRange(_)
                ) {
                    self.focus_pane(Pane::Sidebar);
                }
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                let snap = tab.snapshot.clone();
                let Update { task, output } = tab.sidebar.update_with(m, snap.as_deref());
                let task = task.map(Message::Sidebar);
                match output {
                    Some(o) => Task::batch([task, self.on_sidebar(o)]),
                    None => task,
                }
            }
            Message::Changes(m) => {
                if matches!(
                    m,
                    changes::Message::Select(..) | changes::Message::RightClick(..)
                ) {
                    self.focus_pane(Pane::Changes);
                }
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                let snap = tab.snapshot.clone();
                let Update { task, output } = tab.changes.update_with(m, snap.as_deref());
                let task = task.map(Message::Changes);
                match output {
                    Some(o) => Task::batch([task, self.on_changes(o)]),
                    None => task,
                }
            }
            Message::History(m) => {
                // Shift-click selects a range, ⌘-click adds/removes one.
                let m = match m {
                    history::Message::Select(oid) if self.modifiers.shift() => {
                        history::Message::SelectRange(oid)
                    }
                    history::Message::Select(oid) if self.modifiers.command() => {
                        history::Message::ToggleMark(oid)
                    }
                    other => other,
                };
                if matches!(
                    m,
                    history::Message::Select(_)
                        | history::Message::SelectRange(_)
                        | history::Message::ToggleMark(_)
                        | history::Message::RightClick(_)
                ) {
                    self.focus_pane(Pane::History);
                }
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                // Picking a commit in the history (click or arrow keys)
                // asks for its changes: blame gives way. A blame line
                // revealing its commit selects without this message, so
                // blame stays open then.
                if matches!(
                    m,
                    history::Message::Select(_)
                        | history::Message::SelectRange(_)
                        | history::Message::ToggleMark(_)
                ) {
                    tab.blame = None;
                }
                let snap = tab.snapshot.clone();
                let Update { task, output } = tab.history.update_with(m, snap.as_deref());
                let task = task.map(Message::History);
                match output {
                    Some(o) => Task::batch([task, self.on_history(o)]),
                    None => task,
                }
            }
            Message::LocalDiff(m) => self.on_diff_message(diff_view::Role::Local, m),
            Message::CommitDiff(m) => self.on_diff_message(diff_view::Role::Commit, m),
            Message::CodeView(role) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.code_view = role;
                }
                Task::none()
            }
            Message::Commit(m) => {
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                let Update { task, output } = tab.commit.update(m);
                let task = task.map(Message::Commit);
                match output {
                    Some(o) => Task::batch([task, self.on_commit(o)]),
                    None => task,
                }
            }
            Message::Dialog(m) => {
                let Some(dialog) = self.dialog.as_mut() else {
                    return Task::none();
                };
                match dialog.update(m) {
                    Some(outcome) => self.on_dialog_outcome(outcome),
                    None => Task::none(),
                }
            }

            Message::Core(CoreEvent::Command(e)) => {
                self.log.on_command_event(&e);
                Task::none()
            }
            Message::Core(CoreEvent::WorkTreeChanged { repo, paths }) => {
                let parts = self.worktree_parts(repo, &paths);
                self.request_touched(repo, parts, crate::repo_tab::Touched::paths(&paths))
            }
            Message::Core(CoreEvent::IndexChanged(repo)) => {
                self.request_parts(repo, crate::repo_tab::Parts::STATUS)
            }
            // A checkout or fetch can also move submodules' recorded commits.
            Message::Core(CoreEvent::RefsChanged(repo)) => self.request_parts(
                repo,
                crate::repo_tab::Parts {
                    refs: true,
                    modules: true,
                    ..crate::repo_tab::Parts::STATUS
                },
            ),
            Message::Core(CoreEvent::ConfigChanged(repo)) => self.request_parts(
                repo,
                crate::repo_tab::Parts {
                    config: true,
                    ..crate::repo_tab::Parts::STATUS
                },
            ),
            Message::Core(_) => Task::none(),
            Message::Key(k) => self.on_shortcut(k),
            Message::KeyInput(k) => self.on_key_input(k),
            Message::FocusPane(pane) => {
                self.focus_pane(pane);
                Task::none()
            }
            Message::Menu(m) => match self.menu.as_mut() {
                Some(menu) => {
                    let outcome = menu.update(m);
                    self.on_menu_outcome(outcome)
                }
                None => Task::none(),
            },
            Message::Tick => {
                self.tick = self.tick.wrapping_add(1);
                self.log.tick();
                Task::none()
            }
            Message::FileHovered => {
                self.drop_hover = true;
                Task::none()
            }
            Message::FileHoverLeft => {
                self.drop_hover = false;
                Task::none()
            }
            Message::FileDropped(path) => {
                self.drop_hover = false;
                // Dropping a file inside a repository opens that repository.
                let dir = if path.is_dir() {
                    path
                } else {
                    path.parent().map(Path::to_path_buf).unwrap_or(path)
                };
                self.update(Message::Open(dir))
            }

            Message::Open(path) => {
                self.opening += 1;
                let runner = self.runner.clone();
                Task::perform(
                    {
                        let path = path.clone();
                        async move {
                            Repository::open(&path, runner)
                                .await
                                .map(Box::new)
                                .map_err(|e| e.to_string())
                        }
                    },
                    move |result| Message::Opened(path.clone(), result),
                )
            }
            Message::Opened(path, result) => {
                self.opening = self.opening.saturating_sub(1);
                match result {
                    Ok(repo) => self.add_tab(*repo),
                    Err(e) => {
                        let shown = widgets::format::tilde(&path);
                        self.log.note(
                            NoteKind::Error,
                            format!("can't open {shown}"),
                            vec![e, "not a git repository? create one with `init`, or `clone <url>` into it".into()],
                        );
                        self.flash(NoteKind::Error, format!("✗ not a git repository: {shown}"));
                        Task::none()
                    }
                }
            }
            Message::Restored(repos, active) => {
                let tasks: Vec<_> = repos.into_iter().map(|r| self.add_tab(r)).collect();
                if let Some(a) = active.filter(|a| *a < self.tabs.len()) {
                    self.active = a;
                }
                self.note_active();
                Task::batch(tasks)
            }
            // Tests cannot show the system's folder chooser: it is cancelled.
            Message::PickFolder if cfg!(test) => Task::done(Message::Picked(None)),
            Message::PickFolder => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Open repository")
                        .pick_folder()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                Message::Picked,
            ),
            Message::OpenPicker => self.open_picker(),
            Message::Frame => Task::none(),
            Message::Picker(m) => match self.picker.as_mut() {
                Some(p) => {
                    let Update { task, output } = p.update(m);
                    let task = task.map(Message::Picker);
                    match output {
                        Some(out) => Task::batch([task, self.on_picker_output(out)]),
                        None => task,
                    }
                }
                None => Task::none(),
            },
            Message::Picked(Some(path)) => self.update(Message::Open(path)),
            Message::Picked(None) => self.restore_focus(),
            Message::Loaded(id, result) => self.on_loaded(id, result),
            Message::HistoryLoaded(id, request, rev, result) => {
                let Some(tab) = self
                    .tab_mut(id)
                    .filter(|t| t.history_request == request && t.history.revision() == Some(&rev))
                else {
                    return Task::none();
                };
                match result {
                    Ok(h) => tab.history.loaded(&rev, h),
                    Err(e) => {
                        tab.history.load_failed();
                        self.log
                            .note(NoteKind::Error, format!("history of {rev}"), vec![e]);
                    }
                }
                Task::none()
            }
            Message::DiffFinished(id, role, done) => {
                if let Some(tab) = self.tab_mut(id) {
                    return tab
                        .diff_mut(role)
                        .finish(done)
                        .map(move |next| Message::DiffFinished(id, role, next));
                }
                Task::none()
            }
            Message::AmendTextLoaded(id, message) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.commit.set_last_message(message);
                }
                Task::none()
            }
            Message::SyncProgress(p) => {
                if let Some((sync, _)) = self.sync.as_mut() {
                    sync.progress = Some(p);
                }
                Task::none()
            }
            Message::OpDone {
                repo,
                label,
                kind,
                result,
                step,
            } => {
                let repo = match &kind {
                    OpKind::Checkout(git_dir) | OpKind::DeleteBranches(git_dir) => self
                        .tabs
                        .iter()
                        .find(|tab| tab.repo.handle().git_dir == *git_dir)
                        .map_or(repo, |tab| tab.repo.id()),
                    _ => repo,
                };
                let recorded = step.is_some();
                if let Some(step) = step {
                    self.record_step(repo, *step);
                }
                self.on_op_done(repo, label, kind, result, recorded)
            }
            Message::IntegrateDone {
                repo,
                label,
                result,
                record,
            } => self.on_integrate_done(repo, label, result, record),
            Message::UndoChecked { repo, redo, now } => self.on_undo_checked(repo, redo, now),
            Message::DiscardMeasured {
                repo,
                tracked,
                untracked,
                fits,
            } => self.show_discard(repo, tracked, untracked, fits),
            Message::CommitLocated { repo, oid, result } => {
                self.on_commit_located(repo, oid, result)
            }
            Message::SwitchMeasured {
                repo,
                name,
                remote,
                from,
                changed,
                blocked,
            } => self.show_switch(repo, name, remote, from, changed, blocked),
            Message::Banner(m) => self.on_banner(m),
            Message::Conflict(m) => self.on_conflict_message(m),
            Message::Blame(m) => self.on_blame_message(m),
            Message::BlameLoaded(id, request, path, result) => {
                let Some(b) = self
                    .tab_mut(id)
                    .filter(|tab| tab.blame_request == request)
                    .and_then(|t| t.blame.as_mut())
                else {
                    return Task::none();
                };
                b.loaded(&path, result);
                // Show the lines right away, colors follow when ready.
                let Some(lines) = b.lines().filter(|_| b.path == path) else {
                    return Task::none();
                };
                let text: Vec<String> = lines.iter().map(|l| l.content.clone()).collect();
                Task::perform(
                    crate::features::blame_view::highlight(path.clone(), text),
                    move |spans| Message::BlameHighlighted(id, request, path.clone(), spans),
                )
            }
            Message::BlameHighlighted(id, request, path, spans) => {
                if let (Some(b), Some(spans)) = (
                    self.tab_mut(id)
                        .filter(|tab| tab.blame_request == request)
                        .and_then(|t| t.blame.as_mut()),
                    spans,
                ) {
                    b.highlighted(&path, spans);
                }
                Task::none()
            }
            Message::ConflictLoaded(id, path, result) => {
                if let Some(c) = self.tab_mut(id).and_then(|t| t.conflict.as_mut()) {
                    c.loaded(&path, result);
                }
                Task::none()
            }
            Message::BisectDone(id, result) => self.on_bisect_done(id, result),
            Message::RebasePlanLoaded(id, base, plan, squash) => {
                self.on_rebase_plan(id, base, plan, squash)
            }
            Message::RebaseEditor(m) => self.on_rebase_editor(m),
            Message::SquashPreviewed {
                repo,
                ids,
                selected,
                result,
            } => self.on_squash_preview(repo, ids, selected, result),
            Message::Undo => self.undo_steps(false, 1),
            Message::Redo => self.undo_steps(true, 1),
            Message::UndoList(m) => self.on_undo_list(m),
            Message::ResizeListStart => {
                self.list_drag_start = Some(self.settings.appearance.list_cols);
                Task::none()
            }
            Message::ResizeList(dx) => {
                let start = *self
                    .list_drag_start
                    .get_or_insert(self.settings.appearance.list_cols);
                self.settings.appearance.list_cols =
                    resized_list_cols(start, dx, self.tokens.cols(1));
                Task::none()
            }
            Message::ResizeListDone => {
                self.list_drag_start = None;
                self.save_settings();
                Task::none()
            }
            Message::ResizeListReset => {
                self.settings.appearance.list_cols = gf_config::DEFAULT_LIST_COLS;
                self.save_settings();
                Task::none()
            }
            Message::GitFinished {
                repo,
                args,
                result,
                open_after,
            } => self.on_git_finished(repo, args, result, open_after),
        }
    }

    fn on_diff_message(&mut self, role: diff_view::Role, m: diff_view::Message) -> Task<Message> {
        // ⌘-press adds to the line selection, ⇧-press extends it (as in
        // the history list).
        let m = match m {
            diff_view::Message::LineClicked(key, p)
                if p.button == iced::mouse::Button::Left && self.modifiers.command() =>
            {
                diff_view::Message::LinePressedAdd((key, p.caret))
            }
            diff_view::Message::LineClicked(key, p)
                if p.button == iced::mouse::Button::Left && self.modifiers.shift() =>
            {
                diff_view::Message::LinePressedRange((key, p.caret))
            }
            other => other,
        };
        match m {
            diff_view::Message::SelectFile(_) | diff_view::Message::FileMenu(_) => {
                self.focus_pane(Pane::Files);
            }
            diff_view::Message::LineClicked(..)
            | diff_view::Message::LinePressedAdd(_)
            | diff_view::Message::LinePressedRange(_) => self.focus_pane(Pane::Diff),
            _ => {}
        }
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        let wrap = match role {
            diff_view::Role::Local => Message::LocalDiff,
            diff_view::Role::Commit => Message::CommitDiff,
        };
        let Update { task, output } = tab.diff_mut(role).update(m);
        let task = task.map(wrap);
        match output {
            Some(o) => Task::batch([task, self.on_diff(role, o)]),
            None => task,
        }
    }

    fn add_tab(&mut self, repo: Repository) -> Task<Message> {
        // Already open (possibly via another path inside it): just switch.
        if let Some(i) = self.tabs.iter().position(|t| t.repo.cwd() == repo.cwd()) {
            return self.select_tab(i);
        }
        let cwd = repo.cwd().to_path_buf();
        let handle = repo.handle();
        let watcher = gf_watch::RepoWatcher::start(
            repo.id(),
            handle.workdir.clone(),
            handle.git_dir.clone(),
            self.bus.clone(),
        )
        .map_err(|e| {
            self.log.note(
                NoteKind::Error,
                "file watching unavailable",
                vec![
                    e.to_string(),
                    "changes made outside gf need ⌘R to show up".into(),
                ],
            );
        })
        .ok();
        let repo = repo.with_gix_fetch(self.settings.git.gix_fetch);
        // A new tab queues its first load; `load_due` starts it once it is
        // the tab's turn (after every message).
        self.dismiss_repo_overlays();
        self.tabs.push(RepoTab::new(repo, watcher));
        self.active = self.tabs.len() - 1;
        self.note_active();
        self.settings.remember_repo(&cwd);
        self.save_settings();
        self.flash(
            NoteKind::Info,
            format!("opened {}", widgets::format::tilde(&cwd)),
        );
        self.focus_pane(Pane::History);
        Task::none()
    }

    /// A pending repository open can switch tabs even while a modal is visible.
    fn dismiss_repo_overlays(&mut self) {
        self.menu = None;
        self.code_dialog = None;
        let _ = self.handle(Message::Dialog(dialog::Message::Cancel));
    }

    fn select_tab(&mut self, i: usize) -> Task<Message> {
        if i < self.tabs.len() && i != self.active {
            self.dismiss_repo_overlays();
            self.active = i;
            self.note_active();
            self.save_settings();
        }
        self.reveal_active_tab()
    }

    /// Remembers the active tab as the most recently used.
    fn note_active(&mut self) {
        if let Some(id) = self.active_tab().map(|t| t.repo.id()) {
            self.mru.touch(id);
        }
    }

    fn mru_order(&self) -> Vec<usize> {
        let ids: Vec<_> = self.tabs.iter().map(|t| t.repo.id()).collect();
        self.mru.order(&ids)
    }

    /// Scrolls the tab bar so the active tab is in view.
    fn reveal_active_tab(&self) -> Task<Message> {
        let n = self.tabs.len();
        if n < 2 {
            return Task::none();
        }
        #[allow(clippy::cast_precision_loss)]
        let x = self.active as f32 / (n - 1) as f32;
        operation::snap_to(
            tabs::BAR_ID,
            iced::widget::scrollable::RelativeOffset {
                x: Some(x),
                y: None,
            },
        )
    }

    /// ⌃⇥: shows the switcher on the previous tab, or moves along it.
    fn cycle_tab(&mut self, back: bool) -> Task<Message> {
        let n = self.tabs.len();
        if n < 2 {
            self.flash(NoteKind::Info, "only one tab open — ⌘O opens another");
            return Task::none();
        }
        self.switcher = Some(tab_switcher::step(self.switcher, n, back));
        // A ⌃⇥ that arrives after ⌃ was already let go switches at once.
        if !self.modifiers.control() {
            return self.finish_switch();
        }
        Task::none()
    }

    fn finish_switch(&mut self) -> Task<Message> {
        let Some(pos) = self.switcher.take() else {
            return Task::none();
        };
        match self.mru_order().get(pos) {
            Some(&i) => self.select_tab(i),
            None => Task::none(),
        }
    }

    fn tab_keys(&self) -> Vec<tab_list::Key> {
        self.tabs.iter().map(tab_list::Key::of).collect()
    }

    /// ⌘P / `▾ N tabs`: opens the tab dropdown, or closes it when open.
    fn toggle_tab_list(&mut self) -> Task<Message> {
        if self.tab_list.take().is_some() {
            return self.restore_focus();
        }
        if self.tabs.len() < 2 {
            self.flash(NoteKind::Info, "only one tab open — ⌘O opens another");
            return Task::none();
        }
        self.menu = None;
        self.tab_list = Some(tab_list::State::new(&self.tab_keys(), self.active));
        operation::focus(tab_list::FILTER_ID)
    }

    pub(super) fn close_tab_list(&mut self) -> Task<Message> {
        self.tab_list = None;
        self.restore_focus()
    }

    fn on_tab_list(&mut self, m: tab_list::Message) -> Task<Message> {
        let keys = self.tab_keys();
        let Some(list) = self.tab_list.as_mut() else {
            return Task::none();
        };
        match m {
            tab_list::Message::Filter(f) => {
                list.set_filter(f, &keys);
                Task::none()
            }
            tab_list::Message::Scroll(_) => {
                list.update(&m);
                Task::none()
            }
            tab_list::Message::Pick(i) => {
                let close = self.close_tab_list();
                Task::batch([close, self.select_tab(i)])
            }
            tab_list::Message::CloseTab(i) => self.close_tab_from_list(i),
            tab_list::Message::Dismiss => self.close_tab_list(),
        }
    }

    /// Closes a tab from the dropdown; the dropdown stays open on the
    /// remaining tabs (closes itself when one is left).
    pub(super) fn close_tab_from_list(&mut self, i: usize) -> Task<Message> {
        let task = self.close_tab(i);
        if self.tabs.len() < 2 {
            return Task::batch([task, self.close_tab_list()]);
        }
        let keys = self.tab_keys();
        if let Some(list) = self.tab_list.as_mut() {
            list.refilter(&keys);
        }
        Task::batch([task, operation::focus(tab_list::FILTER_ID)])
    }

    /// Shows the open-repository window with the recent repositories.
    fn open_picker(&mut self) -> Task<Message> {
        self.menu = None;
        self.tab_list = None;
        let entries = self
            .settings
            .repos
            .recent
            .iter()
            .map(|path| {
                let tab = self.tabs.iter().position(|t| t.repo.cwd() == path);
                repo_picker::Entry::read(path.clone(), tab)
            })
            .collect();
        self.picker = Some(repo_picker::State::new(entries));
        operation::focus(repo_picker::FILTER_ID)
    }

    fn on_picker_output(&mut self, out: repo_picker::Output) -> Task<Message> {
        use repo_picker::Output;
        match out {
            Output::Open(path) => {
                self.picker = None;
                Task::batch([self.restore_focus(), self.update(Message::Open(path))])
            }
            Output::SwitchTab(i) => {
                self.picker = None;
                Task::batch([self.restore_focus(), self.select_tab(i)])
            }
            Output::Forget(path) => {
                let shown = widgets::format::tilde(&path);
                self.confirm(dialogs::Confirm::change(
                    "forget-recent",
                    "Forget repository",
                    vec![
                        format!("Remove {shown} from the recent list?"),
                        "The folder and its files are not touched; open it again to bring it back."
                            .into(),
                    ],
                    "Forget",
                    Purpose::ForgetRecent(path),
                ))
            }
            Output::Completed => Task::batch([
                operation::focus(repo_picker::FILTER_ID),
                operation::move_cursor_to_end(repo_picker::FILTER_ID),
            ]),
            Output::Browse => {
                self.picker = None;
                self.update(Message::PickFolder)
            }
            Output::Close => {
                self.picker = None;
                self.restore_focus()
            }
        }
    }

    /// Removes `path` from the recent list (and from the open window).
    fn forget_recent(&mut self, path: &Path) -> Task<Message> {
        self.settings.repos.recent.retain(|p| p != path);
        self.save_settings();
        if let Some(p) = self.picker.as_mut() {
            p.forgotten(path);
        }
        self.flash(
            NoteKind::Info,
            format!("forgot {}", widgets::format::tilde(path)),
        );
        self.restore_focus()
    }

    fn close_tab(&mut self, i: usize) -> Task<Message> {
        if i >= self.tabs.len() {
            return Task::none();
        }
        if i == self.active {
            self.dismiss_repo_overlays();
        }
        let closed = self.tabs.remove(i);
        let git_dir = &closed.repo.handle().git_dir;
        if self
            .checkouts
            .get(git_dir)
            .is_some_and(|c| c.phase != CheckoutPhase::Running)
        {
            self.checkouts.remove(git_dir);
        }
        if self.active > i || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        // The tab now shown counts as used (⌃⇥ order).
        self.note_active();
        self.flash(
            NoteKind::Info,
            format!("closed {} — reopen from Recent or ⌘O", closed.name()),
        );
        self.save_settings();
        self.restore_focus()
    }

    /// Opens an action menu where the mouse was last pressed. The prompt
    /// gives up focus so the menu's letter keys are not typed into it.
    pub(super) fn open_menu(&mut self, menu: context_menu::Menu<Message>) -> Task<Message> {
        self.menu = Some(if self.menu_by_key {
            context_menu::State::centered(menu)
        } else {
            context_menu::State::open(self.pointer.last_press(), menu)
        });
        iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus())
    }

    fn close_menu(&mut self) -> Task<Message> {
        self.menu = None;
        self.restore_focus()
    }

    fn on_menu_outcome(&mut self, outcome: context_menu::Outcome<Message>) -> Task<Message> {
        match outcome {
            context_menu::Outcome::None => Task::none(),
            context_menu::Outcome::Close => self.close_menu(),
            context_menu::Outcome::Run(m) => {
                self.menu = None;
                let action = self.update(m);
                Task::batch([self.restore_focus(), action])
            }
        }
    }

    fn on_shortcut(&mut self, k: Shortcut) -> Task<Message> {
        if self.menu.is_some() {
            return match k {
                Shortcut::Escape => self.close_menu(),
                // Everything else waits until the menu is closed.
                _ => Task::none(),
            };
        }
        // While a dialog is open only its keys apply.
        if self.dialog.is_some() {
            return match k {
                Shortcut::Escape => self.update(Message::Dialog(dialog::Message::Cancel)),
                Shortcut::Enter => match self.dialog.as_mut().and_then(Dialog::enter) {
                    Some(outcome) => self.on_dialog_outcome(outcome),
                    None => Task::none(),
                },
                _ => Task::none(),
            };
        }
        if self.settings_open {
            return match k {
                Shortcut::Settings | Shortcut::Escape => self.toggle_settings(),
                Shortcut::Quit => iced::exit(),
                _ => Task::none(),
            };
        }
        match k {
            Shortcut::Open => self.open_picker(),
            Shortcut::Settings => self.toggle_settings(),
            Shortcut::Forget => Task::none(),
            Shortcut::CycleTab { back } => self.cycle_tab(back),
            Shortcut::TabList => self.toggle_tab_list(),
            Shortcut::Undo => self.undo_steps(false, 1),
            Shortcut::Redo => self.undo_steps(true, 1),
            Shortcut::PrevTab | Shortcut::NextTab => {
                let n = self.tabs.len();
                if n < 2 {
                    return Task::none();
                }
                let by = if k == Shortcut::PrevTab { n - 1 } else { 1 };
                self.select_tab((self.active + by) % n)
            }
            Shortcut::CloseTab => self.close_tab(self.active),
            Shortcut::Refresh => self.refresh_active(),
            Shortcut::SelectTab(i) => self.select_tab(i),
            Shortcut::FocusPrompt => self.focus_prompt(),
            Shortcut::ToggleRefs => {
                self.settings.appearance.show_refs = !self.settings.appearance.show_refs;
                self.save_settings();
                Task::none()
            }
            Shortcut::ToggleHistory => {
                self.settings.appearance.show_history = !self.settings.appearance.show_history;
                self.save_settings();
                self.active_tab()
                    .filter(|tab| {
                        self.settings.appearance.show_history
                            && tab.code_view == diff_view::Role::Commit
                    })
                    .map_or_else(Task::none, |tab| {
                        tab.history.restore_scroll().map(Message::History)
                    })
            }
            // ⌘E opens/closes the working tree (swaps the main area mode).
            Shortcut::ToggleLocal => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.code_view = match tab.code_view {
                        diff_view::Role::Commit => diff_view::Role::Local,
                        diff_view::Role::Local => diff_view::Role::Commit,
                    };
                }
                Task::none()
            }
            Shortcut::ToggleLog => {
                self.settings.appearance.show_command_log =
                    !self.settings.appearance.show_command_log;
                self.save_settings();
                if self.settings.appearance.show_command_log {
                    self.log.restore_scroll().map(Message::Log)
                } else {
                    Task::none()
                }
            }
            Shortcut::HistoryPrev => self.update(Message::Prompt(prompt::Message::HistoryPrev)),
            Shortcut::HistoryNext => self.update(Message::Prompt(prompt::Message::HistoryNext)),
            Shortcut::Complete => self.update(Message::Prompt(prompt::Message::Complete)),
            Shortcut::Commit => self.update(Message::Commit(commit_box::Message::Submit {
                amend: false,
            })),
            Shortcut::Escape if self.rebase_editor().is_some() => {
                self.on_rebase_editor(crate::features::rebase_editor::Message::Cancel)
            }
            Shortcut::Escape => {
                if let Some((_, token)) = &self.sync {
                    token.cancel();
                    self.flash(NoteKind::Info, "cancelling…");
                }
                Task::none()
            }
            Shortcut::Enter | Shortcut::FindInFile | Shortcut::FindMatch { .. } => Task::none(),
            Shortcut::CodeView(role) => self.update(Message::CodeView(role)),
            Shortcut::Copy => self.copy_diff_selection(),
            Shortcut::SelectAll => {
                // Only the diff being worked in: ⌘A elsewhere selects nothing.
                match (self.focus, self.active_tab().map(|t| t.code_view)) {
                    (Pane::Diff, Some(role)) => {
                        self.on_diff_message(role, diff_view::Message::SelectAll)
                    }
                    _ => Task::none(),
                }
            }
            Shortcut::Quit => iced::exit(),
        }
    }

    // --------------------------------------------------------------- view

    pub fn view(&self) -> Element<'_, Message> {
        let t = &self.tokens;
        let tab = self.active_tab();
        let cx = Cx {
            tokens: t,
            snapshot: tab.and_then(|tab| tab.snapshot.as_deref()),
            all_branches: tab.is_some_and(|t| t.all_branches),
        };
        let spinner = widgets::spinner_frame(self.tick);
        let checkout = tab.and_then(|tab| self.checkouts.get(&tab.repo.handle().git_dir));
        let checkout_indicator = checkout.map(|c| (c.target.as_str(), spinner));

        let body: Element<'_, Message> = match tab {
            None => {
                welcome::view(t, &self.settings.repos.recent, self.drop_hover).map(Message::Welcome)
            }
            Some(tab) => {
                let appearance = &self.settings.appearance;
                let active = self.pane();
                let left: Element<'_, Message> = if appearance.show_refs {
                    container(focus::pane_frame(
                        t,
                        Pane::Sidebar,
                        active,
                        tab.sidebar
                            .view_with_checkout(cx, checkout_indicator)
                            .map(Message::Sidebar),
                    ))
                    .width(Length::Fixed(t.font_size * 22.0))
                    .into()
                } else {
                    widgets::collapsed_strip(
                        t,
                        "»",
                        "REFS",
                        "show refs ⌘B".into(),
                        Message::Key(Shortcut::ToggleRefs),
                    )
                };
                use crate::features::op_banner;
                let op = cx.snapshot.and_then(|s| {
                    op_banner::view(t, &s.state, s.conflict_count(), tab.bisect.as_ref())
                });
                // Changes gf stashed when the user left this branch.
                let left_here = cx
                    .snapshot
                    .filter(|s| s.state.is_clean())
                    .and_then(|s| s.left_here())
                    .filter(|e| !tab.left_dismissed.contains(&e.id))
                    .map(|e| op_banner::left_changes(t, e));
                let mut main = column![].spacing(4);
                for banner in op.into_iter().chain(left_here) {
                    main = main.push(banner.map(Message::Banner));
                }
                let main: Element<'_, Message> = main
                    .push(main_area(
                        t,
                        tab,
                        cx,
                        active,
                        appearance.list_cols,
                        appearance.show_history,
                    ))
                    .into();
                row![left, main].spacing(4).height(Length::Fill).into()
            }
        };

        let mut layout = column![
            tabs::view(t, &self.tabs, self.active, self.tab_list.is_some()).map(Message::Tabs)
        ]
        .height(Length::Fill);
        if let Some(tab) = tab {
            layout = layout.push(
                toolbar::view(t, tab, self.sync.as_ref().map(|(s, _)| s), spinner)
                    .map(Message::Toolbar),
            );
        }
        layout = layout.push(
            container(body)
                .padding(Padding::from([4, 4]))
                .height(Length::Fill),
        );
        if self.settings.appearance.show_command_log {
            layout = layout.push(
                container(self.log.view(cx).map(Message::Log))
                    .padding(Padding::from([0, 4]))
                    .height(Length::Fixed(t.font_size * 11.0)),
            );
        } else {
            layout = layout.push(
                container(
                    self.log
                        .collapsed_view(t, Message::Key(Shortcut::ToggleLog)),
                )
                .padding(Padding::from([0, 4])),
            );
        }
        layout = layout
            .push(
                container(self.prompt.view(cx).map(Message::Prompt))
                    .padding(Padding::from([2, 10])),
            )
            .push(status_line::view(
                t,
                tab,
                self.busy().then_some((
                    spinner,
                    checkout.map(|c| {
                        let target = &c.target;
                        target
                            .strip_prefix("refs/heads/")
                            .or_else(|| target.strip_prefix("refs/remotes/"))
                            .unwrap_or(target)
                    }),
                )),
                self.flash.as_ref(),
                tab.map(|tab| status_line::UndoBar {
                    undo: tab.undo.next(false).map(|s| s.label.clone()),
                    redo: tab.undo.next(true).map(|s| s.label.clone()),
                    on_undo: Message::Undo,
                    on_redo: Message::Redo,
                    on_list: Message::UndoList(crate::features::undo_list::Message::Toggle),
                }),
            ));

        let base = container(layout)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(style::app(t));

        // A confirmation sits above the window (picker, rebase editor) that
        // asked for it.
        let modal: Option<Element<'_, Message>> =
            match (&self.dialog, &self.picker, self.rebase_editor()) {
                (Some(d), _, _) => Some(d.view(t).map(Message::Dialog)),
                _ if self.code_dialog.is_some() => self
                    .code_dialog
                    .as_ref()
                    .map(|code| code.view(cx).map(Message::CodeDialog)),
                _ if self.settings_open => Some(settings_view::view(t).map(Message::Settings)),
                (None, Some(p), _) => Some(p.view(cx).map(Message::Picker)),
                _ if self.switcher.is_some() => Some(
                    tab_switcher::view(
                        t,
                        &self.tabs,
                        &self.mru_order(),
                        self.switcher.unwrap_or(0),
                    )
                    .map(Message::Switcher),
                ),
                (None, None, Some(e)) => Some(e.view(t).map(Message::RebaseEditor)),
                (None, None, None) => None,
            };
        // Clicking the shade around the open window closes it.
        let close_on_shade =
            self.dialog.is_none() && self.code_dialog.is_none() && self.picker.is_some();
        // The layer structure never changes: switching the root between
        // `base` and `stack![base, …]` would rebuild the widget tree and
        // reset every scroll position (lists jumped to the top whenever a
        // menu or dialog opened). Empty layers are plain spaces.
        let empty = || -> Element<'_, Message> { iced::widget::space().into() };
        let modal_layer: Element<'_, Message> = match modal {
            None => empty(),
            Some(modal) => {
                let shade = Color {
                    a: 0.55,
                    ..Color::BLACK
                };
                let backdrop = container(iced::widget::space())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(Background::Color(shade)),
                        ..container::Style::default()
                    });
                let backdrop = if close_on_shade {
                    iced::widget::mouse_area(backdrop)
                        .on_press(Message::Picker(repo_picker::Message::Close))
                        .into()
                } else {
                    Element::from(backdrop)
                };
                opaque(stack![backdrop, center(opaque(modal))])
            }
        };
        let undo_list = self
            .undo_list
            .as_ref()
            .zip(self.active_tab().map(|t| &t.undo));
        let menu_layer: Element<'_, Message> = match (&self.menu, &self.tab_list, undo_list) {
            (Some(menu), _, _) => opaque(menu.view(t).map(Message::Menu)),
            (None, Some(list), _) => {
                opaque(list.view(t, &self.tabs, self.active).map(Message::TabList))
            }
            (None, None, Some((list, history))) => {
                opaque(list.view(t, history).map(Message::UndoList))
            }
            (None, None, None) => empty(),
        };
        let screen: Element<'_, Message> = stack![base, modal_layer, menu_layer].into();
        self.pointer.track(screen)
    }

    // ------------------------------------------------------- subscriptions

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run_with(BusSubscription(self.bus.clone()), |bus| {
                tokio_stream::wrappers::BroadcastStream::new(bus.0.subscribe())
                    .filter_map(|e| async move { e.ok() })
                    .map(Message::Core)
            }),
            event::listen_with(global_event),
        ];
        // The open window shakes when it refuses a path: redraw each frame.
        if self
            .picker
            .as_ref()
            .is_some_and(repo_picker::State::shaking)
        {
            subs.push(window::frames().map(|_| Message::Frame));
        }
        if self.busy() {
            subs.push(iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick));
        } else if !self.tabs.is_empty() {
            // Keeps relative times (e.g. `⟳ fetch (5m)`) current while idle.
            subs.push(iced::time::every(Duration::from_secs(30)).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }
}

/// The main area, in one of two modes chosen by the tabs on top (⌘E):
///
/// ```text
/// [◆ HISTORY] [● WORKING TREE · 3]      [◆ HISTORY] [● WORKING TREE · 3]
/// ┌ HISTORY ─────────────────┐           ┌ WORKING TREE ─────┬ COMMIT ┐
/// ├ CHANGES (commit) ────────┤           ├ CHANGES (local) ──┴────────┤
/// └──────────────────────────┘           └────────────────────────────┘
/// ```
///
/// There is one CHANGES view; its color (blue = a commit, yellow = your
/// local work) and the active tab say which changes are shown. Each mode
/// keeps its own selection and scroll position.
/// Width of the file list column (columns of text), the same in HISTORY
/// (commit files) and WORKING TREE (changed files), so the divider to the
/// diff stays put when switching between them.
/// Narrowest and widest the file list beside the diff can be dragged
/// (character columns).
const MIN_LIST_COLS: f32 = 18.0;
const MAX_LIST_COLS: f32 = 120.0;

/// The list width after dragging the divider `dx` pixels from a drag that
/// started at `start` columns.
fn resized_list_cols(start: f32, dx: f32, col_px: f32) -> f32 {
    (start + dx / col_px).clamp(MIN_LIST_COLS, MAX_LIST_COLS)
}

/// The file list beside the diff, the divider to resize it and the diff:
/// one width for HISTORY and WORKING TREE.
fn list_split<'a>(
    t: &'a Tokens,
    list_cols: f32,
    list: Element<'a, Message>,
    diff: Element<'a, Message>,
) -> Element<'a, Message> {
    let divider = widgets::splitter::Splitter::new(Message::ResizeList, t.border, t.accent())
        .on_press(Message::ResizeListStart)
        .on_release(Message::ResizeListDone)
        .on_reset(Message::ResizeListReset);
    row![
        container(list).width(Length::Fixed(t.cols(1) * list_cols)),
        divider,
        container(diff).width(Length::Fill),
    ]
    .height(Length::Fill)
    .into()
}

fn main_area<'a>(
    t: &'a Tokens,
    tab: &'a RepoTab,
    cx: Cx<'a>,
    active: Pane,
    list_cols: f32,
    show_history: bool,
) -> Element<'a, Message> {
    let frame =
        |pane: Pane, content: Element<'a, Message>| focus::pane_frame(t, pane, active, content);
    use diff_view::Role;
    let changed = cx.snapshot.map_or(0, |s| {
        s.status
            .entries
            .iter()
            .filter(|e| !matches!(e, gf_core::model::StatusEntry::Ignored { .. }))
            .count()
    });
    let mode_tab = |role: Role, label: String, hint: &str| {
        widgets::tip(
            t,
            iced::widget::button(
                widgets::mono(t, label).wrapping(iced::widget::text::Wrapping::None),
            )
            .padding(Padding::from([2, 10]))
            .style(style::code_tab(t, role.accent(t), tab.code_view == role))
            .on_press(Message::CodeView(role)),
            hint.into(),
        )
    };
    let working_label = if changed == 0 {
        "● WORKING TREE · clean".to_owned()
    } else {
        format!("● WORKING TREE · {changed} changed")
    };
    let open_close = match tab.code_view {
        Role::Commit => "open the working tree ⌘E",
        Role::Local => "back to history ⌘E",
    };
    let tabs = row![
        mode_tab(
            Role::Commit,
            "◆ HISTORY".into(),
            "history and what each commit changed ⌘["
        ),
        mode_tab(
            Role::Local,
            working_label,
            "your uncommitted work: stage, commit ⌘]"
        ),
        widgets::dim(t, format!("  {open_close}")),
    ]
    .spacing(2)
    .align_y(iced::Alignment::Center);

    match tab.code_view {
        Role::Commit => {
            let top = if show_history {
                container(frame(
                    Pane::History,
                    tab.history.view(cx).map(Message::History),
                ))
                .height(Length::FillPortion(2))
            } else {
                container(widgets::collapsed_bar(
                    t,
                    widgets::colored(t, "▾ HISTORY", t.accent()),
                    "show history ⌘⇧H",
                    Message::Key(Shortcut::ToggleHistory),
                ))
            };
            let changes: Element<'a, Message> = match &tab.blame {
                Some(b) => frame(Pane::Diff, b.view(t).map(Message::Blame)),
                None => list_split(
                    t,
                    list_cols,
                    frame(
                        Pane::Files,
                        tab.commit_diff.files_view(cx).map(Message::CommitDiff),
                    ),
                    frame(
                        Pane::Diff,
                        tab.commit_diff.view(cx).map(Message::CommitDiff),
                    ),
                ),
            };
            column![tabs, top, container(changes).height(Length::FillPortion(3)),]
                .spacing(4)
                .width(Length::Fill)
                .into()
        }
        // WORKING TREE: changed files over the commit box on the left, the
        // selected file's changes on the right.
        Role::Local => {
            let left = column![
                container(frame(
                    Pane::Changes,
                    tab.changes.view(cx).map(Message::Changes)
                ))
                .height(Length::FillPortion(3)),
                container(tab.commit.view(cx).map(Message::Commit)).height(Length::FillPortion(2)),
            ]
            .spacing(4);
            let diff = frame(
                Pane::Diff,
                match &tab.conflict {
                    Some(c) => c.view(t).map(Message::Conflict),
                    None => tab.local_diff.view(cx).map(Message::LocalDiff),
                },
            );
            column![tabs, list_split(t, list_cols, left.into(), diff),]
                .spacing(4)
                .width(Length::Fill)
                .into()
        }
    }
}

/// Identity for the event-bus subscription (there is only one bus).
struct BusSubscription(EventBus);

impl Hash for BusSubscription {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "gf-event-bus".hash(state);
    }
}

/// All global input handling. Uses `listen_with` (not `keyboard::listen`)
/// so ⌘ shortcuts work even while a text field has focus and captured the
/// key — the prompt is focused most of the time.
fn global_event(event: iced::Event, status: event::Status, _id: window::Id) -> Option<Message> {
    match event {
        iced::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => Some(Message::Modifiers(m)),
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            modifiers,
            physical_key,
            ..
        }) => {
            let input = focus::KeyInput {
                shortcut: shortcut(&key, modifiers, physical_key, status),
                menu: menu_key(&key, modifiers, physical_key),
                nav: nav_key(&key, modifiers, physical_key),
            };
            (input.shortcut.is_some() || input.menu.is_some() || input.nav.is_some())
                .then_some(Message::KeyInput(input))
        }
        // Native dialogs can swallow the key-up of ⌘; never keep it stuck.
        iced::Event::Window(window::Event::Focused | window::Event::Unfocused) => {
            Some(Message::Modifiers(keyboard::Modifiers::empty()))
        }
        iced::Event::Window(window::Event::FileHovered(_)) => Some(Message::FileHovered),
        iced::Event::Window(window::Event::FilesHoveredLeft) => Some(Message::FileHoverLeft),
        iced::Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
        _ => None,
    }
}

/// Keys an open menu understands: letters (by physical key, so the Korean
/// input method works too), ↑/↓ and Enter, without modifiers.
fn menu_key(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    physical: keyboard::key::Physical,
) -> Option<context_menu::Key> {
    use keyboard::key::{Key, Named};
    if !modifiers.is_empty() {
        return None;
    }
    match key.as_ref() {
        Key::Named(Named::ArrowUp) => Some(context_menu::Key::Up),
        Key::Named(Named::ArrowDown) => Some(context_menu::Key::Down),
        Key::Named(Named::Enter) => Some(context_menu::Key::Enter),
        _ => key
            .to_latin(physical)
            .filter(char::is_ascii_alphabetic)
            .map(context_menu::Key::Char),
    }
}

/// Keys that move around panels: arrows, paging, Enter, Space, ⇥ (with
/// or without Shift) and plain symbols/letters (by physical key).
fn nav_key(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    physical: keyboard::key::Physical,
) -> Option<Nav> {
    use keyboard::key::{Key as K, Named};
    let shift = modifiers.shift();
    if modifiers.command() || modifiers.alt() || modifiers.control() {
        return None;
    }
    let key = match key.as_ref() {
        K::Named(Named::ArrowUp) => Key::Up,
        K::Named(Named::ArrowDown) => Key::Down,
        K::Named(Named::ArrowLeft) => Key::Left,
        K::Named(Named::ArrowRight) => Key::Right,
        K::Named(Named::PageUp) => Key::PageUp,
        K::Named(Named::PageDown) => Key::PageDown,
        K::Named(Named::Home) => Key::Home,
        K::Named(Named::End) => Key::End,
        K::Named(Named::Enter) => Key::Enter,
        K::Named(Named::Space) => Key::Space,
        K::Named(Named::Tab) => Key::Tab,
        _ if shift => return None,
        _ => match key.to_latin(physical)? {
            ' ' => Key::Space,
            c if c.is_ascii_graphic() => Key::Char(c.to_ascii_lowercase()),
            _ => return None,
        },
    };
    Some(Nav { key, shift })
}

fn shortcut(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    physical: keyboard::key::Physical,
    status: event::Status,
) -> Option<Shortcut> {
    use keyboard::key::{Code, Key, Named, Physical};
    // ⌃⇥ (Ctrl+Tab everywhere; on macOS ⌘ is a different key).
    if modifiers.control()
        && !modifiers.logo()
        && let Key::Named(Named::Tab) = key.as_ref()
    {
        return Some(Shortcut::CycleTab {
            back: modifiers.shift(),
        });
    }
    if modifiers.command() {
        if physical == Physical::Code(Code::Comma) && !modifiers.shift() && !modifiers.alt() {
            return Some(Shortcut::Settings);
        }
        match key.as_ref() {
            Key::Named(Named::Enter) => return Some(Shortcut::Commit),
            Key::Named(Named::Backspace) => return Some(Shortcut::Forget),
            _ => {}
        }
        // Layout-independent: ⌘O works with the Korean input method too.
        // ⌘V/X are left to text fields; ⌘C/A/Z too while one has focus
        // (see `focus`).
        let edit = status == event::Status::Captured;
        return Some(match key.to_latin(physical)? {
            // Only when no text widget used them for its own text.
            'c' if !edit => Shortcut::Copy,
            'a' if !edit => Shortcut::SelectAll,
            'f' if !modifiers.shift() && !modifiers.alt() => Shortcut::FindInFile,
            'g' | 'G' if !modifiers.alt() => Shortcut::FindMatch {
                back: modifiers.shift(),
            },
            'o' => Shortcut::Open,
            'z' | 'Z' if modifiers.shift() => Shortcut::Redo,
            'z' => Shortcut::Undo,
            'p' => Shortcut::TabList,
            'w' => Shortcut::CloseTab,
            'r' => Shortcut::Refresh,
            'l' => Shortcut::FocusPrompt,
            'j' => Shortcut::ToggleLog,
            'h' | 'H' if modifiers.shift() && !modifiers.alt() => Shortcut::ToggleHistory,
            'b' => Shortcut::ToggleRefs,
            'e' => Shortcut::ToggleLocal,
            // ⌘⇧[ / ⌘⇧]: previous / next repository tab (Safari, Terminal).
            '[' | '{' if modifiers.shift() => Shortcut::PrevTab,
            ']' | '}' if modifiers.shift() => Shortcut::NextTab,
            // Left tab / right tab of the main area.
            '[' => Shortcut::CodeView(diff_view::Role::Commit),
            ']' => Shortcut::CodeView(diff_view::Role::Local),
            'q' => Shortcut::Quit,
            d @ '1'..='9' => Shortcut::SelectTab(d as usize - '1' as usize),
            _ => return None,
        });
    }
    if let Key::Named(Named::Escape) = key.as_ref() {
        return Some(Shortcut::Escape);
    }
    // Plain keys only when no widget used them (e.g. Enter in a text field
    // is that field's submit).
    if status == event::Status::Captured || !modifiers.is_empty() {
        return None;
    }
    match key.as_ref() {
        Key::Named(Named::Enter) => Some(Shortcut::Enter),
        Key::Named(Named::ArrowUp) => Some(Shortcut::HistoryPrev),
        Key::Named(Named::ArrowDown) => Some(Shortcut::HistoryNext),
        Key::Named(Named::Tab) => Some(Shortcut::Complete),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_the_divider_follows_the_cursor_within_limits() {
        // 8px columns: 80px right of the start is 10 columns wider.
        assert!((resized_list_cols(34.0, 80.0, 8.0) - 44.0).abs() < f32::EPSILON);
        assert!((resized_list_cols(34.0, -1000.0, 8.0) - MIN_LIST_COLS).abs() < f32::EPSILON);
        assert!((resized_list_cols(34.0, 5000.0, 8.0) - MAX_LIST_COLS).abs() < f32::EPSILON);
        // Coming back from past a limit lands where the cursor is.
        assert!((resized_list_cols(34.0, -16.0, 8.0) - 32.0).abs() < f32::EPSILON);
    }
    use keyboard::key::{Code, Key, Named, Physical};

    fn press(
        key: Key,
        code: Code,
        modifiers: keyboard::Modifiers,
        status: event::Status,
    ) -> Option<Shortcut> {
        shortcut(&key, modifiers, Physical::Code(code), status)
    }

    #[test]
    fn undo_keys_work_with_any_input_method() {
        let cmd = keyboard::Modifiers::COMMAND;
        // Korean input: the key reports 'ㅋ', the physical key is Z.
        assert_eq!(
            press(
                Key::Character("ㅋ".into()),
                Code::KeyZ,
                cmd,
                event::Status::Ignored
            ),
            Some(Shortcut::Undo)
        );
        assert_eq!(
            press(
                Key::Character("Z".into()),
                Code::KeyZ,
                cmd | keyboard::Modifiers::SHIFT,
                event::Status::Ignored
            ),
            Some(Shortcut::Redo)
        );
    }

    #[test]
    fn history_toggle_uses_shift_command_and_the_physical_key() {
        let cmd = keyboard::Modifiers::COMMAND;
        assert_eq!(
            press(
                Key::Character("ㅗ".into()),
                Code::KeyH,
                cmd,
                event::Status::Captured
            ),
            None
        );
        assert_eq!(
            press(
                Key::Character("ㅗ".into()),
                Code::KeyH,
                cmd | keyboard::Modifiers::SHIFT,
                event::Status::Captured
            ),
            Some(Shortcut::ToggleHistory)
        );
    }

    #[test]
    fn tab_switching_keys() {
        let ctrl = keyboard::Modifiers::CTRL;
        let tab = Key::Named(Named::Tab);
        assert_eq!(
            press(tab.clone(), Code::Tab, ctrl, event::Status::Captured),
            Some(Shortcut::CycleTab { back: false })
        );
        assert_eq!(
            press(
                tab,
                Code::Tab,
                ctrl | keyboard::Modifiers::SHIFT,
                event::Status::Ignored
            ),
            Some(Shortcut::CycleTab { back: true })
        );
        let cmd_shift = keyboard::Modifiers::COMMAND | keyboard::Modifiers::SHIFT;
        assert_eq!(
            press(
                Key::Character("{".into()),
                Code::BracketLeft,
                cmd_shift,
                event::Status::Ignored
            ),
            Some(Shortcut::PrevTab)
        );
        assert_eq!(
            press(
                Key::Character("}".into()),
                Code::BracketRight,
                cmd_shift,
                event::Status::Ignored
            ),
            Some(Shortcut::NextTab)
        );
        // Without ⇧ the brackets still switch the main area's mode.
        assert_eq!(
            press(
                Key::Character("[".into()),
                Code::BracketLeft,
                keyboard::Modifiers::COMMAND,
                event::Status::Ignored
            ),
            Some(Shortcut::CodeView(diff_view::Role::Commit))
        );
    }

    #[test]
    fn settings_shortcut_uses_the_physical_comma_key() {
        assert_eq!(
            press(
                Key::Character(",".into()),
                Code::Comma,
                keyboard::Modifiers::COMMAND,
                event::Status::Captured,
            ),
            Some(Shortcut::Settings)
        );
    }

    #[test]
    fn command_shortcuts_work_even_when_a_field_captured_the_key() {
        let cmd = keyboard::Modifiers::COMMAND;
        let o = Key::Character("o".into());
        assert_eq!(
            press(o.clone(), Code::KeyO, cmd, event::Status::Captured),
            Some(Shortcut::Open)
        );
        assert_eq!(
            press(
                Key::Character("2".into()),
                Code::Digit2,
                cmd,
                event::Status::Captured
            ),
            Some(Shortcut::SelectTab(1))
        );
        // Korean input method: the character is ㅐ but the physical key is O.
        assert_eq!(
            press(
                Key::Character("ㅐ".into()),
                Code::KeyO,
                cmd,
                event::Status::Ignored
            ),
            Some(Shortcut::Open)
        );
        // Clipboard keys stay with the text field.
        assert_eq!(
            press(
                Key::Character("c".into()),
                Code::KeyC,
                cmd,
                event::Status::Captured
            ),
            None
        );
        // Nobody used ⌘C / ⌘A: they copy / select the diff lines.
        assert_eq!(
            press(
                Key::Character("c".into()),
                Code::KeyC,
                cmd,
                event::Status::Ignored
            ),
            Some(Shortcut::Copy)
        );
        assert_eq!(
            press(
                Key::Character("ㅁ".into()),
                Code::KeyA,
                cmd,
                event::Status::Ignored
            ),
            Some(Shortcut::SelectAll)
        );
    }

    #[test]
    fn plain_keys_only_when_not_captured() {
        let none = keyboard::Modifiers::empty();
        let enter = Key::Named(Named::Enter);
        assert_eq!(
            press(enter.clone(), Code::Enter, none, event::Status::Captured),
            None
        );
        assert_eq!(
            press(enter, Code::Enter, none, event::Status::Ignored),
            Some(Shortcut::Enter)
        );
        assert_eq!(
            press(
                Key::Named(Named::Escape),
                Code::Escape,
                none,
                event::Status::Captured
            ),
            Some(Shortcut::Escape)
        );
        assert_eq!(
            press(
                Key::Character("o".into()),
                Code::KeyO,
                none,
                event::Status::Ignored
            ),
            None
        );
    }

    #[test]
    fn menu_keys_are_plain_letters_arrows_and_enter() {
        let none = keyboard::Modifiers::empty();
        let k = |key: Key, code: Code, m| menu_key(&key, m, keyboard::key::Physical::Code(code));
        assert_eq!(
            k(Key::Character("p".into()), Code::KeyP, none),
            Some(context_menu::Key::Char('p'))
        );
        // Korean input method: physical key decides.
        assert_eq!(
            k(Key::Character("ㅔ".into()), Code::KeyP, none),
            Some(context_menu::Key::Char('p'))
        );
        assert_eq!(
            k(Key::Named(Named::ArrowDown), Code::ArrowDown, none),
            Some(context_menu::Key::Down)
        );
        assert_eq!(
            k(
                Key::Character("p".into()),
                Code::KeyP,
                keyboard::Modifiers::COMMAND
            ),
            None
        );
        assert_eq!(k(Key::Character("1".into()), Code::Digit1, none), None);
    }

    #[test]
    fn nav_keys_are_arrows_paging_and_plain_symbols() {
        let none = keyboard::Modifiers::empty();
        let k = |key: Key, code: Code, m| nav_key(&key, m, keyboard::key::Physical::Code(code));
        assert_eq!(
            k(
                Key::Named(Named::ArrowDown),
                Code::ArrowDown,
                keyboard::Modifiers::SHIFT
            ),
            Some(Nav {
                key: crate::nav::Key::Down,
                shift: true
            })
        );
        // Korean input method: `ㅇ` is the D key.
        assert_eq!(
            k(Key::Character("ㅇ".into()), Code::KeyD, none),
            Some(Nav {
                key: crate::nav::Key::Char('d'),
                shift: false
            })
        );
        assert_eq!(
            k(Key::Character("/".into()), Code::Slash, none).map(|n| n.key),
            Some(crate::nav::Key::Char('/'))
        );
        // ⌘ keys are shortcuts, never navigation.
        assert_eq!(
            k(
                Key::Named(Named::ArrowDown),
                Code::ArrowDown,
                keyboard::Modifiers::COMMAND
            ),
            None
        );
        assert_eq!(
            k(
                Key::Character("D".into()),
                Code::KeyD,
                keyboard::Modifiers::SHIFT
            ),
            None
        );
    }

    #[test]
    fn bracket_shortcuts_switch_the_code_view() {
        let cmd = keyboard::Modifiers::COMMAND;
        assert_eq!(
            press(
                Key::Character("[".into()),
                Code::BracketLeft,
                cmd,
                event::Status::Captured
            ),
            Some(Shortcut::CodeView(diff_view::Role::Commit))
        );
        assert_eq!(
            press(
                Key::Character("]".into()),
                Code::BracketRight,
                cmd,
                event::Status::Ignored
            ),
            Some(Shortcut::CodeView(diff_view::Role::Local))
        );
    }
}
