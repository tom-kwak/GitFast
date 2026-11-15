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

mod dialogs;
mod ops;
mod outputs;

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
use crate::features::command_log::NoteKind;
use crate::features::dialog::{self, Dialog};
use crate::features::diff_view::DiffSource;
use crate::features::prompt::PROMPT_ID;
use crate::features::status_line::Flash;
use crate::features::{
    changes, command_log, commit_box, diff_view, history, prompt, sidebar, status_line, tabs,
    toolbar, welcome,
};
use crate::repo_tab::{History, RepoTab, Snapshot};
use crate::theme::{self, Tokens, style};
use crate::widgets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Open,
    CloseTab,
    Refresh,
    SelectTab(usize),
    FocusPrompt,
    ToggleLog,
    ToggleRefs,
    ToggleLocal,
    HistoryPrev,
    HistoryNext,
    Complete,
    Commit,
    Escape,
    Enter,
    /// ⌘[ / ⌘]: HISTORY / WORKING TREE mode of the main area.
    CodeView(diff_view::Role),
    Quit,
}

/// What to do after an operation finished, besides refreshing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpKind {
    Generic,
    Commit,
    Fetch,
    Push { branch: Option<String> },
    Pull,
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

    Core(CoreEvent),
    Key(Shortcut),
    Tick,
    FileHovered,
    FileHoverLeft,
    FileDropped(PathBuf),
    /// ⌘ (Ctrl elsewhere) pressed or released.
    CommandHeld(bool),

    Open(PathBuf),
    Opened(PathBuf, Result<Box<Repository>, String>),
    Restored(Vec<Repository>, Option<usize>),
    PickFolder,
    Picked(Option<PathBuf>),
    Loaded(RepoId, Result<Arc<Snapshot>, String>),
    HistoryLoaded(RepoId, String, Result<Arc<History>, String>),
    DiffLoaded(
        RepoId,
        DiffSource,
        Result<Arc<diff_view::LoadedDiff>, String>,
    ),
    DiffHighlighted(RepoId, DiffSource, usize, Arc<diff_view::FileSpans>),
    AmendTextLoaded(RepoId, Option<String>),
    SyncProgress(Progress),
    OpDone {
        repo: RepoId,
        label: String,
        kind: OpKind,
        result: Result<(), String>,
    },
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
    bus: EventBus,
    runner: GitRunner,
    tabs: Vec<RepoTab>,
    active: usize,
    prompt: prompt::State,
    log: command_log::State,
    dialog: Option<Dialog<Purpose>>,
    /// Running fetch/pull/push and its cancel handle.
    sync: Option<(toolbar::Sync, CancellationToken)>,
    flash: Option<Flash>,
    tick: usize,
    drop_hover: bool,
    /// While ⌘ is held, characters a text field inserts are shortcut keys,
    /// not typing (see `Message::Prompt`).
    command_held: bool,
    /// Repositories being opened (for the busy indicator).
    opening: usize,
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
        let active = settings.repos.active;
        let restore = if to_restore.is_empty() {
            Task::none()
        } else {
            let runner = runner.clone();
            Task::perform(
                async move {
                    let mut repos = Vec::new();
                    for path in to_restore {
                        match Repository::open(&path, runner.clone()).await {
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
            bus,
            runner,
            tabs: Vec::new(),
            active: 0,
            prompt: prompt::State::default(),
            log: command_log::State::default(),
            dialog: None,
            sync: None,
            flash: None,
            tick: 0,
            drop_hover: false,
            command_held: false,
            opening: 0,
        };
        (app, Task::batch([restore, operation::focus(PROMPT_ID)]))
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

    // ------------------------------------------------------------- update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tabs(m) => match m {
                tabs::Message::Select(i) => self.select_tab(i),
                tabs::Message::Close(i) => self.close_tab(i),
                tabs::Message::Open => self.update(Message::PickFolder),
            },
            Message::Toolbar(m) => self.on_toolbar(m),
            Message::Welcome(m) => match m {
                welcome::Message::OpenRecent(p) => self.update(Message::Open(p)),
                welcome::Message::PickFolder => self.update(Message::PickFolder),
            },
            Message::CommandHeld(held) => {
                self.command_held = held;
                Task::none()
            }
            // ⌘O must open a folder, not type "o" into the prompt.
            Message::Prompt(prompt::Message::Input(_)) if self.command_held => Task::none(),
            Message::Prompt(m) => {
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
                let _ = self.log.update(m);
                Task::none()
            }
            Message::Sidebar(m) => {
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
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                let Update { task, output } = tab.changes.update(m);
                let task = task.map(Message::Changes);
                match output {
                    Some(o) => Task::batch([task, self.on_changes(o)]),
                    None => task,
                }
            }
            Message::History(m) => {
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                let Update { task, output } = tab.history.update(m);
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
            Message::Core(
                CoreEvent::WorkTreeChanged { repo, .. }
                | CoreEvent::IndexChanged(repo)
                | CoreEvent::RefsChanged(repo)
                | CoreEvent::ConfigChanged(repo),
            ) => self.request_refresh(repo),
            Message::Core(_) => Task::none(),
            Message::Key(k) => self.on_shortcut(k),
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
                Task::batch(tasks)
            }
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
            Message::Picked(Some(path)) => self.update(Message::Open(path)),
            Message::Picked(None) => operation::focus(PROMPT_ID),
            Message::Loaded(id, result) => self.on_loaded(id, result),
            Message::HistoryLoaded(id, rev, result) => {
                let Some(tab) = self.tab_mut(id) else {
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
            Message::DiffLoaded(id, source, result) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.diff_mut(source.role()).loaded(&source, result);
                }
                Task::none()
            }
            Message::DiffHighlighted(id, source, file, spans) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.diff_mut(source.role())
                        .highlighted(&source, file, spans);
                }
                Task::none()
            }
            Message::AmendTextLoaded(id, message) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.commit.set_amend_message(message);
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
            } => self.on_op_done(repo, label, kind, result),
            Message::GitFinished {
                repo,
                args,
                result,
                open_after,
            } => self.on_git_finished(repo, args, result, open_after),
        }
    }

    fn on_diff_message(&mut self, role: diff_view::Role, m: diff_view::Message) -> Task<Message> {
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
            self.active = i;
            return Task::none();
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
        let mut tab = RepoTab::new(repo, watcher);
        let load = ops::load_snapshot(&mut tab);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.settings.remember_repo(&cwd);
        self.save_settings();
        self.flash(
            NoteKind::Info,
            format!("opened {}", widgets::format::tilde(&cwd)),
        );
        Task::batch([load, operation::focus(PROMPT_ID)])
    }

    fn select_tab(&mut self, i: usize) -> Task<Message> {
        if i < self.tabs.len() && i != self.active {
            self.active = i;
            self.save_settings();
        }
        Task::none()
    }

    fn close_tab(&mut self, i: usize) -> Task<Message> {
        if i >= self.tabs.len() {
            return Task::none();
        }
        let closed = self.tabs.remove(i);
        if self.active > i || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.flash(
            NoteKind::Info,
            format!("closed {} — reopen from Recent or ⌘O", closed.name()),
        );
        self.save_settings();
        operation::focus(PROMPT_ID)
    }

    fn on_shortcut(&mut self, k: Shortcut) -> Task<Message> {
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
        match k {
            Shortcut::Open => self.update(Message::PickFolder),
            Shortcut::CloseTab => self.close_tab(self.active),
            Shortcut::Refresh => self.refresh_active(),
            Shortcut::SelectTab(i) => self.select_tab(i),
            Shortcut::FocusPrompt => operation::focus(PROMPT_ID),
            Shortcut::ToggleRefs => {
                self.settings.appearance.show_refs = !self.settings.appearance.show_refs;
                self.save_settings();
                Task::none()
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
                Task::none()
            }
            Shortcut::HistoryPrev => self.update(Message::Prompt(prompt::Message::HistoryPrev)),
            Shortcut::HistoryNext => self.update(Message::Prompt(prompt::Message::HistoryNext)),
            Shortcut::Complete => self.update(Message::Prompt(prompt::Message::Complete)),
            Shortcut::Commit => self.update(Message::Commit(commit_box::Message::Submit)),
            Shortcut::Escape => {
                if let Some((_, token)) = &self.sync {
                    token.cancel();
                    self.flash(NoteKind::Info, "cancelling…");
                }
                Task::none()
            }
            Shortcut::Enter => Task::none(),
            Shortcut::CodeView(role) => self.update(Message::CodeView(role)),
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

        let body: Element<'_, Message> = match tab {
            None => {
                welcome::view(t, &self.settings.repos.recent, self.drop_hover).map(Message::Welcome)
            }
            Some(tab) => {
                let appearance = &self.settings.appearance;
                let toggle = |m: toolbar::Message| Message::Toolbar(m);
                let left: Element<'_, Message> = if appearance.show_refs {
                    container(tab.sidebar.view(cx).map(Message::Sidebar))
                        .width(Length::Fixed(t.font_size * 22.0))
                        .into()
                } else {
                    widgets::collapsed_strip(
                        t,
                        "»",
                        "REFS",
                        "show refs ⌘B".into(),
                        toggle(toolbar::Message::ToggleRefs),
                    )
                };
                row![left, main_area(t, tab, cx)]
                    .spacing(4)
                    .height(Length::Fill)
                    .into()
            }
        };

        let mut layout = column![].height(Length::Fill);
        if !self.tabs.is_empty() {
            layout = layout.push(tabs::view(t, &self.tabs, self.active).map(Message::Tabs));
        }
        if let Some(tab) = tab {
            layout = layout.push(
                toolbar::view(
                    t,
                    tab,
                    self.sync.as_ref().map(|(s, _)| s),
                    spinner,
                    toolbar::Panels {
                        refs: self.settings.appearance.show_refs,
                    },
                )
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
        }
        layout = layout
            .push(
                container(self.prompt.view(cx).map(Message::Prompt))
                    .padding(Padding::from([2, 10])),
            )
            .push(status_line::view(
                t,
                tab,
                self.busy().then_some(spinner),
                self.flash.as_ref(),
            ));

        let base = container(layout)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(style::app(t));

        match &self.dialog {
            None => base.into(),
            Some(d) => {
                let shade = Color {
                    a: 0.55,
                    ..Color::BLACK
                };
                let overlay = container(center(d.view(t).map(Message::Dialog)))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(Background::Color(shade)),
                        ..container::Style::default()
                    });
                stack![base, opaque(overlay)].into()
            }
        }
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
        if self.busy() {
            subs.push(iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick));
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
fn main_area<'a>(t: &'a Tokens, tab: &'a RepoTab, cx: Cx<'a>) -> Element<'a, Message> {
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

    let (top, changes): (Element<'a, Message>, Element<'a, Message>) = match tab.code_view {
        Role::Commit => (
            tab.history.view(cx).map(Message::History),
            row![
                container(tab.commit_diff.files_view(cx).map(Message::CommitDiff))
                    .width(Length::Fixed(t.font_size * 20.0)),
                tab.commit_diff.view(cx).map(Message::CommitDiff),
            ]
            .spacing(4)
            .into(),
        ),
        Role::Local => (
            row![
                tab.changes.view(cx).map(Message::Changes),
                container(tab.commit.view(cx).map(Message::Commit))
                    .width(Length::Fixed(t.font_size * 34.0)),
            ]
            .spacing(4)
            .into(),
            tab.local_diff.view(cx).map(Message::LocalDiff),
        ),
    };
    column![
        tabs,
        container(top).height(Length::FillPortion(2)),
        container(changes).height(Length::FillPortion(3)),
    ]
    .spacing(4)
    .width(Length::Fill)
    .into()
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
        iced::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
            Some(Message::CommandHeld(m.command()))
        }
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            modifiers,
            physical_key,
            ..
        }) => shortcut(&key, modifiers, physical_key, status).map(Message::Key),
        // Native dialogs can swallow the key-up of ⌘; never keep it stuck.
        iced::Event::Window(window::Event::Focused | window::Event::Unfocused) => {
            Some(Message::CommandHeld(false))
        }
        iced::Event::Window(window::Event::FileHovered(_)) => Some(Message::FileHovered),
        iced::Event::Window(window::Event::FilesHoveredLeft) => Some(Message::FileHoverLeft),
        iced::Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
        _ => None,
    }
}

fn shortcut(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    physical: keyboard::key::Physical,
    status: event::Status,
) -> Option<Shortcut> {
    use keyboard::key::{Key, Named};
    if modifiers.command() {
        if let Key::Named(Named::Enter) = key.as_ref() {
            return Some(Shortcut::Commit);
        }
        // Layout-independent: ⌘O works with the Korean input method too.
        // ⌘C/V/X/A/Z are left to text fields.
        return Some(match key.to_latin(physical)? {
            'o' => Shortcut::Open,
            'w' => Shortcut::CloseTab,
            'r' => Shortcut::Refresh,
            'l' => Shortcut::FocusPrompt,
            'j' => Shortcut::ToggleLog,
            'b' => Shortcut::ToggleRefs,
            'e' => Shortcut::ToggleLocal,
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
