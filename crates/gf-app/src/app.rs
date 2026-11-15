//! Root of the UI: owns feature states, routes messages, reacts to feature
//! outputs and lays out the window.
//!
//! ```text
//! ┌ tabs ──────────────────────────────────────────────┐
//! │ refs (sidebar) │ history                           │
//! │                ├───────────────────────────────────┤
//! │                │ changes                           │
//! ├ command log (⌘J) ──────────────────────────────────┤
//! │ gf ❯ prompt                                        │
//! └ status line ───────────────────────────────────────┘
//! ```

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gf_config::{AppPaths, Settings};
use gf_core::model::{Commit, RepoId};
use gf_core::{CoreEvent, EventBus};
use gf_git::{GitCommand, GitRunner, LogQuery, Repository};
use iced::futures::StreamExt;
use iced::widget::{column, container, operation, row};
use iced::{Element, Length, Padding, Subscription, Task, event, keyboard, window};

use crate::feature::{Cx, Feature, Update};
use crate::features::command_log::NoteKind;
use crate::features::prompt::PROMPT_ID;
use crate::features::prompt::parse::{BUILTINS, PromptCommand};
use crate::features::status_line::Flash;
use crate::features::{changes, command_log, history, prompt, sidebar, status_line, tabs, welcome};
use crate::git_actions;
use crate::repo_tab::{RepoTab, Snapshot};
use crate::theme::{self, Tokens, style};
use crate::widgets;

const HISTORY_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Open,
    CloseTab,
    Refresh,
    SelectTab(usize),
    FocusPrompt,
    ToggleLog,
    HistoryPrev,
    HistoryNext,
    Complete,
    Quit,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tabs(tabs::Message),
    Welcome(welcome::Message),
    Prompt(prompt::Message),
    Log(command_log::Message),
    Sidebar(sidebar::Message),
    Changes(changes::Message),
    History(history::Message),

    Core(CoreEvent),
    Key(Shortcut),
    Tick,
    FileHovered,
    FileHoverLeft,
    FileDropped(PathBuf),

    Open(PathBuf),
    Opened(PathBuf, Result<Box<Repository>, String>),
    Restored(Vec<Repository>, Option<usize>),
    PickFolder,
    Picked(Option<PathBuf>),
    Loaded(RepoId, Result<Arc<Snapshot>, String>),
    HistoryLoaded(RepoId, String, Result<Arc<Vec<Commit>>, String>),
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
    flash: Option<Flash>,
    tick: usize,
    drop_hover: bool,
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
            flash: None,
            tick: 0,
            drop_hover: false,
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
        self.opening > 0 || self.tabs.iter().any(|t| t.loading) || self.log.has_running()
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
            Message::Welcome(m) => match m {
                welcome::Message::OpenRecent(p) => self.update(Message::Open(p)),
                welcome::Message::PickFolder => self.update(Message::PickFolder),
            },
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
                let Update { task, output } = tab.sidebar.update(m);
                let task = task.map(Message::Sidebar);
                match output {
                    Some(sidebar::Output::ShowRevision(rev)) => {
                        tab.history.set_revision(rev.clone());
                        let load = rev.map(|r| load_history(tab, r)).unwrap_or_else(Task::none);
                        Task::batch([task, load])
                    }
                    None => task,
                }
            }
            Message::Changes(m) => {
                if let Some(tab) = self.active_tab_mut() {
                    let _ = tab.changes.update(m);
                }
                Task::none()
            }
            Message::History(m) => {
                if let Some(tab) = self.active_tab_mut() {
                    let _ = tab.history.update(m);
                }
                Task::none()
            }

            Message::Core(CoreEvent::Command(e)) => {
                self.log.on_command_event(&e);
                Task::none()
            }
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
                            vec![
                                e,
                                "not a git repository? create one with `init`, or `clone <url>` into it".into(),
                            ],
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
            Message::Loaded(id, result) => {
                let Some(tab) = self.tab_mut(id) else {
                    return Task::none();
                };
                tab.loading = false;
                match result {
                    Ok(snap) => {
                        tab.sidebar.retain_existing(&snap);
                        tab.snapshot = Some(snap);
                        tab.error = None;
                        // Keep a selected non-HEAD history in sync with the refresh.
                        if tab.sidebar.selected().is_none() && tab.history.revision().is_some() {
                            tab.history.set_revision(None);
                        }
                        match tab.history.revision().map(str::to_owned) {
                            Some(rev) => load_history(tab, rev),
                            None => Task::none(),
                        }
                    }
                    Err(e) => {
                        tab.error = Some(e.clone());
                        self.log.note(NoteKind::Error, "refresh failed", vec![e]);
                        Task::none()
                    }
                }
            }
            Message::HistoryLoaded(id, rev, result) => {
                let Some(tab) = self.tab_mut(id) else {
                    return Task::none();
                };
                match result {
                    Ok(commits) => tab.history.set_commits(&rev, commits),
                    Err(e) => {
                        tab.history.load_failed();
                        self.log
                            .note(NoteKind::Error, format!("history of {rev}"), vec![e]);
                    }
                }
                Task::none()
            }
            Message::GitFinished {
                repo,
                args,
                result,
                open_after,
            } => {
                let title = format!("git {}", args.join(" "));
                let mut tasks = Vec::new();
                match result {
                    Ok(()) => {
                        self.flash(
                            NoteKind::Info,
                            format!("✓ {}", widgets::format::truncate(&title, 50)),
                        );
                        if let Some(dir) = open_after {
                            tasks.push(self.update(Message::Open(dir)));
                        }
                    }
                    Err(e) => {
                        let mut lines = Vec::new();
                        if let Some(hint) = git_actions::hint_for_error(&args, &e) {
                            lines.push(format!("hint: {hint}"));
                        }
                        if !lines.is_empty() {
                            self.log.note(NoteKind::Info, "what to do next", lines);
                        }
                        self.flash(
                            NoteKind::Error,
                            format!("✗ {} — see log", widgets::format::truncate(&title, 40)),
                        );
                        if !self.settings.appearance.show_command_log {
                            self.settings.appearance.show_command_log = true;
                        }
                    }
                }
                // Any command may have changed the repository: refresh it.
                if let Some(tab) =
                    repo.and_then(|id| self.tabs.iter_mut().find(|t| t.repo.id() == id))
                {
                    tasks.push(load_snapshot(tab));
                }
                Task::batch(tasks)
            }
        }
    }

    fn add_tab(&mut self, repo: Repository) -> Task<Message> {
        // Already open (possibly via another path inside it): just switch.
        if let Some(i) = self.tabs.iter().position(|t| t.repo.cwd() == repo.cwd()) {
            self.active = i;
            return Task::none();
        }
        let cwd = repo.cwd().to_path_buf();
        let mut tab = RepoTab::new(repo);
        let load = load_snapshot(&mut tab);
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
        match k {
            Shortcut::Open => self.update(Message::PickFolder),
            Shortcut::CloseTab => self.close_tab(self.active),
            Shortcut::Refresh => self.refresh_active(),
            Shortcut::SelectTab(i) => self.select_tab(i),
            Shortcut::FocusPrompt => operation::focus(PROMPT_ID),
            Shortcut::ToggleLog => {
                self.settings.appearance.show_command_log =
                    !self.settings.appearance.show_command_log;
                self.save_settings();
                Task::none()
            }
            Shortcut::HistoryPrev => self.update(Message::Prompt(prompt::Message::HistoryPrev)),
            Shortcut::HistoryNext => self.update(Message::Prompt(prompt::Message::HistoryNext)),
            Shortcut::Complete => self.update(Message::Prompt(prompt::Message::Complete)),
            Shortcut::Quit => iced::exit(),
        }
    }

    fn refresh_active(&mut self) -> Task<Message> {
        match self.active_tab_mut() {
            Some(tab) => load_snapshot(tab),
            None => Task::none(),
        }
    }

    fn run_prompt_command(&mut self, cmd: PromptCommand) -> Task<Message> {
        match cmd {
            PromptCommand::Git(args) => self.run_git(args),
            PromptCommand::Open(None) => self.update(Message::PickFolder),
            PromptCommand::Open(Some(path)) => self.update(Message::Open(path)),
            PromptCommand::CloseTab => self.close_tab(self.active),
            PromptCommand::SelectTab(i) => {
                if i >= self.tabs.len() {
                    self.flash(
                        NoteKind::Error,
                        format!("no tab {} — {} open", i + 1, self.tabs.len()),
                    );
                }
                self.select_tab(i)
            }
            PromptCommand::Refresh => self.refresh_active(),
            PromptCommand::Theme(name) => {
                let next = match name {
                    Some(n) if theme::PRESET_NAMES.contains(&n.as_str()) => n,
                    Some(n) => {
                        self.flash(
                            NoteKind::Error,
                            format!(
                                "unknown theme {n} — try: {}",
                                theme::PRESET_NAMES.join(", ")
                            ),
                        );
                        return Task::none();
                    }
                    None => {
                        let i = theme::PRESET_NAMES
                            .iter()
                            .position(|p| *p == self.tokens.name)
                            .map_or(0, |i| (i + 1) % theme::PRESET_NAMES.len());
                        theme::PRESET_NAMES[i].to_owned()
                    }
                };
                self.tokens =
                    theme::preset(&next).with_font_size(self.settings.appearance.font_size);
                self.flash(NoteKind::Info, format!("theme: {next}"));
                self.settings.appearance.theme = next;
                self.save_settings();
                Task::none()
            }
            PromptCommand::ToggleLog => self.on_shortcut(Shortcut::ToggleLog),
            PromptCommand::ClearLog => {
                self.log.clear();
                Task::none()
            }
            PromptCommand::Help => {
                let mut lines: Vec<String> =
                    vec!["Type any git command (the `git` prefix is optional): status, log -5, switch dev …".into()];
                lines.extend(
                    BUILTINS
                        .iter()
                        .map(|(name, help)| format!("{name:<10} {help}")),
                );
                lines.push("↑/↓ history · ⇥ complete · click a ref to see its history".into());
                self.log.note(NoteKind::Info, "help", lines);
                self.settings.appearance.show_command_log = true;
                Task::none()
            }
            PromptCommand::Quit => iced::exit(),
        }
    }

    fn run_git(&mut self, args: Vec<String>) -> Task<Message> {
        let tab = self.active_tab();
        // clone/init run next to the current repository (or in $HOME) and
        // open the result; everything else runs in the active repository.
        let (cwd, repo) = match tab {
            Some(tab) if !git_actions::creates_repository(&args) => {
                (tab.repo.cwd().to_path_buf(), Some(tab.repo.id()))
            }
            Some(tab) => (
                tab.repo
                    .cwd()
                    .parent()
                    .map_or_else(home_dir, Path::to_path_buf),
                None,
            ),
            None => (home_dir(), None),
        };
        if tab.is_none() && !git_actions::creates_repository(&args) && !is_global_command(&args) {
            self.flash(
                NoteKind::Error,
                "no repository open — ⌘O to open one, or `clone <url>`",
            );
        }
        let open_after = git_actions::created_repository(&args, &cwd);
        let mut cmd = GitCommand::new(&args).current_dir(&cwd).echo_stdout();
        if let Some(id) = repo {
            cmd = cmd.repo(id);
        }
        let runner = self.runner.clone();
        if !self.settings.appearance.show_command_log {
            self.flash(NoteKind::Info, "running… output in the command log (⌘J)");
        }
        Task::perform(
            async move {
                runner
                    .run(&cmd)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            },
            move |result| Message::GitFinished {
                repo,
                args: args.clone(),
                result,
                open_after: open_after.clone(),
            },
        )
    }

    // --------------------------------------------------------------- view

    pub fn view(&self) -> Element<'_, Message> {
        let t = &self.tokens;
        let tab = self.active_tab();
        let cx = Cx {
            tokens: t,
            snapshot: tab.and_then(|tab| tab.snapshot.as_deref()),
        };

        let body: Element<'_, Message> = match tab {
            None => {
                welcome::view(t, &self.settings.repos.recent, self.drop_hover).map(Message::Welcome)
            }
            Some(tab) => row![
                container(tab.sidebar.view(cx).map(Message::Sidebar))
                    .width(Length::Fixed(t.font_size * 22.0)),
                column![
                    container(tab.history.view(cx).map(Message::History))
                        .height(Length::FillPortion(3)),
                    container(tab.changes.view(cx).map(Message::Changes))
                        .height(Length::FillPortion(2)),
                ]
                .spacing(4)
                .width(Length::Fill),
            ]
            .spacing(4)
            .height(Length::Fill)
            .into(),
        };

        let mut layout = column![].height(Length::Fill);
        if !self.tabs.is_empty() {
            layout = layout.push(tabs::view(t, &self.tabs, self.active).map(Message::Tabs));
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
                self.busy().then(|| widgets::spinner_frame(self.tick)),
                self.flash.as_ref(),
            ));

        container(layout)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(style::app(t))
            .into()
    }

    // ------------------------------------------------------- subscriptions

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run_with(BusSubscription(self.bus.clone()), |bus| {
                tokio_stream::wrappers::BroadcastStream::new(bus.0.subscribe())
                    .filter_map(|e| async move { e.ok() })
                    .map(Message::Core)
            }),
            keyboard::listen().filter_map(shortcut),
            event::listen_with(window_event),
        ];
        if self.busy() {
            subs.push(iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// Commands that make sense without a repository.
fn is_global_command(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some("--version" | "version" | "help" | "config" | "ls-remote")
    )
}

fn load_snapshot(tab: &mut RepoTab) -> Task<Message> {
    tab.loading = true;
    let repo = tab.repo.clone();
    let id = repo.id();
    Task::perform(Snapshot::load(repo), move |result| {
        Message::Loaded(id, result)
    })
}

fn load_history(tab: &RepoTab, revision: String) -> Task<Message> {
    let repo = tab.repo.clone();
    let id = repo.id();
    let query = LogQuery {
        revisions: vec![revision.clone()],
        max_count: Some(HISTORY_LIMIT),
        ..LogQuery::default()
    };
    Task::perform(
        async move {
            repo.log(&query)
                .await
                .map(Arc::new)
                .map_err(|e| e.to_string())
        },
        move |result| Message::HistoryLoaded(id, revision.clone(), result),
    )
}

/// Identity for the event-bus subscription (there is only one bus).
struct BusSubscription(EventBus);

impl Hash for BusSubscription {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "gf-event-bus".hash(state);
    }
}

fn shortcut(event: keyboard::Event) -> Option<Message> {
    use keyboard::key::{Key, Named};
    let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
        return None;
    };
    let s = match key.as_ref() {
        Key::Character(c) if modifiers.command() => match c {
            "o" => Shortcut::Open,
            "w" => Shortcut::CloseTab,
            "r" => Shortcut::Refresh,
            "l" => Shortcut::FocusPrompt,
            "j" => Shortcut::ToggleLog,
            "q" => Shortcut::Quit,
            d => {
                let n: usize = d.parse().ok().filter(|n| (1..=9).contains(n))?;
                Shortcut::SelectTab(n - 1)
            }
        },
        Key::Named(Named::ArrowUp) if modifiers.is_empty() => Shortcut::HistoryPrev,
        Key::Named(Named::ArrowDown) if modifiers.is_empty() => Shortcut::HistoryNext,
        Key::Named(Named::Tab) if modifiers.is_empty() => Shortcut::Complete,
        _ => return None,
    };
    Some(Message::Key(s))
}

fn window_event(event: iced::Event, _status: event::Status, _id: window::Id) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::FileHovered(_)) => Some(Message::FileHovered),
        iced::Event::Window(window::Event::FilesHoveredLeft) => Some(Message::FileHoverLeft),
        iced::Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
        _ => None,
    }
}
