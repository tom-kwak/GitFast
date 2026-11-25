//! Routes the settings screen and prompt to the app's shared preferences,
//! and logs in to / out of git servers (ACCOUNTS).

use gf_config::DEFAULT_CLONE_ROOT;
use gf_git::ops::credential::{self, Login};
use gf_hosting::{Account, ProviderKind};
use iced::advanced::widget::operation::focusable;
use iced::{Element, Task};

use gf_core::model::RepoId;

use super::dialogs::{Confirm, Purpose};
use super::{App, Message, OpKind, RemoteOp};
use crate::features::command_log::NoteKind;
use crate::features::dialog::{Dialog, Link, Opt, Suggestion};
use crate::features::settings_view::{self, AccountStatus, Probe, Section, ServerUse};
use crate::hosting::{PROVIDERS, provider_label};
use crate::theme::{self, Tokens};

/// Servers offered in the log-in dialog besides the open tabs' own.
const WELL_KNOWN: [&str; 3] = ["github.com", "gitlab.com", "bitbucket.org"];

impl App {
    pub(super) fn toggle_settings(&mut self) -> Task<Message> {
        if self.dialog.is_some()
            || self.picker.is_some()
            || self.switcher.is_some()
            || self.rebase_editor().is_some()
            || self.menu.is_some()
        {
            return Task::none();
        }
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.tab_list = None;
            self.undo_list = None;
            Task::batch([
                iced::advanced::widget::operate(focusable::unfocus()),
                self.refresh_settings_page(false),
            ])
        } else {
            self.restore_focus()
        }
    }

    pub(super) fn settings_view<'a>(
        &'a self,
        t: &'a Tokens,
    ) -> Element<'a, settings_view::Message> {
        settings_view::view(
            t,
            settings_view::Props {
                page: &self.settings_page,
                settings: &self.settings,
                git: gf_core::git_program().display().to_string(),
                git_missing: self.git_missing,
                offer_helper: crate::platform::CREDENTIAL_HELPER,
                system_terminal: crate::platform::TERMINAL,
            },
        )
    }

    pub(super) fn show_settings_section(&mut self, section: Section) -> Task<Message> {
        self.settings_page.section = section;
        Task::none()
    }

    /// Reads again what the settings screen shows from outside gf: the
    /// servers the open tabs use, git's credential helper and whether it
    /// still has each account's token.
    fn refresh_settings_page(&mut self, check_access: bool) -> Task<Message> {
        let (servers, ssh) = self.servers_in_use();
        let page = &mut self.settings_page;
        page.probe_generation += 1;
        let generation = page.probe_generation;
        page.servers = servers;
        page.ssh = ssh;
        page.helper = Probe::Checking;
        page.tokens = self
            .settings
            .accounts
            .iter()
            .map(|a| (a.clone(), Probe::Checking))
            .collect();

        let (protocol, host) = self
            .settings
            .accounts
            .first()
            .map(|a| (a.protocol.clone(), a.host.clone()))
            .or_else(|| {
                page.servers
                    .first()
                    .map(|s| (s.protocol.clone(), s.host.clone()))
            })
            .unwrap_or_else(|| ("https".to_owned(), "github.com".to_owned()));
        page.helper_server = server_label(&protocol, &host);
        let runner = self.runner.clone();
        let helper = Task::perform(
            async move {
                match credential::credential_helper(&runner, &protocol, &host).await {
                    Ok(helper) => Probe::Known(helper),
                    Err(e) => Probe::Failed(e.to_string()),
                }
            },
            move |probe| Message::SettingsProbed(Probed::Helper(generation, probe)),
        );
        let tokens = self.settings.accounts.iter().cloned().map(|account| {
            let runner = self.runner.clone();
            let url = check_access
                .then(|| {
                    page.server(&account.protocol, &account.host)
                        .map(|server| server.url.clone())
                })
                .flatten();
            Task::perform(
                async move {
                    let probe = match probe_account(&runner, &account, url.as_deref()).await {
                        Ok(status) => Probe::Known(status),
                        Err(error) => Probe::Failed(first_line(&error.to_string())),
                    };
                    (account, probe)
                },
                move |(account, probe)| {
                    Message::SettingsProbed(Probed::Token(generation, account, probe))
                },
            )
        });
        let probes = Task::batch(std::iter::once(helper).chain(tokens));
        if check_access {
            probes.chain(Task::done(Message::SettingsProbed(Probed::Refreshed(
                generation,
            ))))
        } else {
            probes
        }
    }

    pub(super) fn settings_probed(&mut self, probed: Probed) -> Task<Message> {
        let page = &mut self.settings_page;
        match probed {
            Probed::Helper(generation, probe) if generation == page.probe_generation => {
                page.helper = probe;
            }
            Probed::HelperSet(Ok(helper)) => {
                page.changing_login = false;
                self.flash(
                    NoteKind::Info,
                    format!("git keeps logins with `{helper}` now"),
                );
                return self.refresh_settings_page(false);
            }
            Probed::HelperSet(Err(e)) => {
                page.changing_login = false;
                self.flash(
                    NoteKind::Error,
                    format!("could not set git's credential helper: {e}"),
                );
            }
            Probed::Token(generation, account, probe) if generation == page.probe_generation => {
                if let Some(entry) = page.tokens.iter_mut().find(|(a, _)| *a == account) {
                    entry.1 = probe;
                }
            }
            Probed::Refreshed(generation) if generation == page.probe_generation => {
                self.flash(NoteKind::Info, "↻ accounts refreshed");
            }
            Probed::Helper(..) | Probed::Token(..) | Probed::Refreshed(_) => {}
        }
        Task::none()
    }

    pub(super) fn on_settings(&mut self, message: settings_view::Message) -> Task<Message> {
        use settings_view::Message as M;
        if !self.settings_open || self.dialog.is_some() {
            return Task::none();
        }
        match message {
            M::Section(section) => self.show_settings_section(section),
            M::SetTheme(name) => {
                self.set_theme(name);
                Task::none()
            }
            M::FontSize(step) => {
                self.step_font_size(step);
                Task::none()
            }
            M::AddAccount
            | M::LogInTo(_)
            | M::UpdateToken(_)
            | M::LogOut(_)
            | M::UseHelper(_)
                if self.settings_page.changing_login =>
            {
                Task::none()
            }
            M::AddAccount => self.login_dialog(LoginAsk::default()),
            M::RefreshAccounts => self.refresh_settings_page(true),
            M::LogInTo(server) => self.login_dialog(LoginAsk {
                server: Some(server),
                ..LoginAsk::default()
            }),
            M::UpdateToken(account) if self.settings.accounts.contains(&account) => {
                self.login_dialog(LoginAsk {
                    server: Some(format!("{}://{}", account.protocol, account.host)),
                    user: Some(account.username),
                    updating: true,
                    ..LoginAsk::default()
                })
            }
            M::LogOut(account) if self.settings.accounts.contains(&account) => {
                self.confirm(Confirm::delete(
                    format!("Remove saved login for {}?", account.username),
                    vec![
                        format!(
                            "Git removes the saved credential for {}.",
                            server_label(&account.protocol, &account.host)
                        ),
                        "This can also affect Git in your terminal. The token stays active on the server."
                            .into(),
                    ],
                    "Remove saved login",
                    Purpose::LogOut(account),
                ))
            }
            M::UpdateToken(_) | M::LogOut(_) => Task::none(),
            M::PickService(host) => self.service_dialog(host),
            M::UseHelper(helper) => self.confirm(
                Confirm::change(
                    "credential-helper",
                    "Let git keep logins?",
                    vec![
                        format!("Runs `git config --global credential.helper {helper}`."),
                        "git then keeps the tokens you log in with, for gf and for git in the \
                         terminal."
                            .into(),
                    ],
                    format!("Use {helper}"),
                    Purpose::UseCredentialHelper(helper.to_owned()),
                )
                .always(),
            ),
            M::EditEditor => self.tool_dialog(Tool::Editor),
            M::EditTerminal => self.tool_dialog(Tool::Terminal),
            M::EditCloneRoot => self.tool_dialog(Tool::CloneRoot),
            M::EditWorktreeRoot => self.tool_dialog(Tool::WorktreeRoot),
            M::ToggleCopyIncluded => {
                let on = !self.settings.worktrees.copy_included;
                self.settings.worktrees.copy_included = on;
                self.save_settings();
                self.flash(
                    NoteKind::Info,
                    if on {
                        "new worktrees get the files .worktreeinclude lists"
                    } else {
                        "new worktrees no longer get .worktreeinclude files"
                    },
                );
                Task::none()
            }
            M::AskAgain(i) => {
                if i < self.settings.confirm.skip.len() {
                    let key = self.settings.confirm.skip.remove(i);
                    self.save_settings();
                    self.flash(
                        NoteKind::Info,
                        format!("asking again before {}", key.replace('-', " ")),
                    );
                }
                Task::none()
            }
            M::AskAgainAll => {
                let n = self.settings.confirm.skip.len();
                self.settings.confirm.skip.clear();
                self.save_settings();
                self.flash(
                    NoteKind::Info,
                    format!("all confirmations restored ({n} were skipped)"),
                );
                Task::none()
            }
            M::Close => self.toggle_settings(),
        }
    }

    fn step_font_size(&mut self, step: i8) {
        let size = self
            .settings
            .appearance
            .font_size
            .saturating_add_signed(step.into());
        let size = size.clamp(
            *settings_view::FONT_SIZES.start(),
            *settings_view::FONT_SIZES.end(),
        );
        self.settings.appearance.font_size = size;
        self.tokens = theme::preset(&self.settings.appearance.theme).with_font_size(size);
        self.flash(NoteKind::Info, format!("font size: {size} pt"));
        self.save_settings();
    }

    /// Asks for a TOOLS setting's new value.
    fn tool_dialog(&mut self, tool: Tool) -> Task<Message> {
        let wt = &self.settings.worktrees;
        let (title, body, label, value, placeholder) = match tool {
            Tool::Editor => (
                "Editor for worktrees",
                "The command that opens a folder in your editor; the folder is added last.",
                "command",
                wt.editor.clone(),
                "code, cursor, idea, subl …",
            ),
            Tool::Terminal => (
                "Terminal for worktrees",
                "The terminal app a worktree opens in. Leave it empty for the system's.",
                "app",
                wt.terminal.clone().unwrap_or_default(),
                crate::platform::TERMINAL,
            ),
            Tool::CloneRoot => (
                "Default workspace",
                "Leave it empty to use ~/gf-workspace.",
                "folder",
                self.settings
                    .repos
                    .clone_root
                    .as_deref()
                    .map(crate::widgets::format::tilde)
                    .unwrap_or_default(),
                DEFAULT_CLONE_ROOT,
            ),
            Tool::WorktreeRoot => (
                "Folder for new worktrees",
                "Leave it empty to put them in <repository>-worktrees/ beside each repository.",
                "folder",
                wt.root
                    .as_deref()
                    .map(crate::widgets::format::tilde)
                    .unwrap_or_default(),
                "~/worktrees",
            ),
        };
        let dialog = Dialog::confirm(title, vec![body.into()], "Save", Purpose::SetTool(tool))
            .field(label, value, placeholder);
        self.open_dialog(dialog)
    }

    pub(super) fn set_tool(&mut self, tool: Tool, value: &str) -> Task<Message> {
        let value = value.trim();
        let wt = &mut self.settings.worktrees;
        let text = match tool {
            Tool::Editor => {
                wt.editor = value.to_owned();
                format!("editor: {value}")
            }
            Tool::Terminal => {
                wt.terminal = (!value.is_empty() && value != "default").then(|| value.to_owned());
                format!(
                    "terminal: {}",
                    wt.terminal.as_deref().unwrap_or(crate::platform::TERMINAL)
                )
            }
            Tool::CloneRoot => {
                self.settings.repos.clone_root =
                    (!value.is_empty()).then(|| crate::widgets::format::expand_tilde(value));
                let root = self.settings.repos.clone_root.as_deref().map_or_else(
                    || DEFAULT_CLONE_ROOT.to_owned(),
                    crate::widgets::format::tilde,
                );
                format!("default clone workspace: {root}")
            }
            Tool::WorktreeRoot => {
                wt.root = (!value.is_empty()).then(|| crate::widgets::format::expand_tilde(value));
                match &wt.root {
                    Some(root) => format!(
                        "new worktrees go in {}",
                        crate::widgets::format::tilde(root)
                    ),
                    None => "new worktrees go beside their repository".to_owned(),
                }
            }
        };
        self.save_settings();
        self.flash(NoteKind::Info, text);
        Task::none()
    }

    pub(super) fn use_credential_helper(&mut self, helper: String) -> Task<Message> {
        self.settings_page.changing_login = true;
        let runner = self.runner.clone();
        Task::perform(
            async move {
                credential::set_credential_helper(&runner, &helper)
                    .await
                    .map(|()| helper)
                    .map_err(|e| e.to_string())
            },
            |result| Message::SettingsProbed(Probed::HelperSet(result)),
        )
    }

    /// The log-in dialog: server (offered from the open tabs' HTTPS
    /// remotes and the big services), user name, token. `ask` presets it:
    /// after a failed attempt, or for a push/pull/fetch the server refused
    /// (then run again once logged in).
    pub(super) fn login_dialog(&mut self, ask: LoginAsk) -> Task<Message> {
        if self.settings_page.changing_login {
            return Task::none();
        }
        let mut servers = self.remote_servers();
        // Prefer the active tab's server when adding an account.
        let server = ask.server.clone().unwrap_or_else(|| {
            servers
                .iter()
                .find(|s| s.hint.starts_with("this tab"))
                .map(|s| s.value.clone())
                .unwrap_or_default()
        });
        for host in WELL_KNOWN {
            let url = format!("https://{host}");
            if servers.iter().any(|s| s.value == url) {
                continue;
            }
            servers.push(Suggestion::new(url, PROVIDERS.guess(host).name()));
        }
        let known = login_server(&server);
        let user =
            ask.user
                .or_else(|| {
                    let (protocol, host) = known.as_ref()?;
                    ask.reason.as_ref()?;
                    let mut users =
                        self.settings.accounts.iter().filter(|account| {
                            account.protocol == *protocol && account.host == *host
                        });
                    let account = users.next()?;
                    users.next().is_none().then(|| account.username.clone())
                })
                .unwrap_or_default();
        let body = ask.reason.into_iter().collect();
        let provider = self.service_of(known.as_ref());
        let confirm = match &ask.retry {
            Some(retry) => format!(
                "Log in and {}",
                crate::widgets::format::truncate(&retry.label, 40)
            ),
            None if ask.updating => "Save token".to_owned(),
            None => "Save login".to_owned(),
        };
        // The first field still empty takes the typing.
        let first_empty = [server.is_empty(), user.is_empty()]
            .iter()
            .position(|empty| *empty)
            .unwrap_or(2);
        let mut dialog = Dialog::confirm(
            if ask.updating {
                "Update Git token"
            } else {
                "Log in to a Git server"
            },
            body,
            confirm,
            Purpose::LogIn(ask.retry.clone()),
        )
        .field("Server URL", server, "https://github.com")
        .suggest(servers, "")
        .quick_suggestions(WELL_KNOWN.len())
        .field("Username", user, "")
        .field("Access token", "", "")
        .secret();
        dialog.set_options_after_field(0, service_row(known.as_ref(), provider));
        if let Some(server) = known.as_ref() {
            dialog = dialog.link(token_link(server, provider));
        }
        dialog.error = ask.error;
        let task = self.open_dialog(dialog);
        match first_empty {
            0 => task,
            i => crate::features::dialog::field_id(i).map_or(task, iced::widget::operation::focus),
        }
    }

    /// The hosting service of a server: picked for its host in the
    /// settings, else guessed from the name. `Other` for no server.
    fn service_of(&self, server: Option<&(String, String)>) -> ProviderKind {
        server.map_or(ProviderKind::Other, |(_, host)| {
            PROVIDERS.for_host(&self.settings.services, host)
        })
    }

    /// Keeps the log-in dialog in step after one of its messages: a
    /// server typed in picks its service again (and shows the service
    /// row only when its name tells nothing), and the token line follows
    /// the server and the service. `server_before` is the server field's
    /// value before the message.
    pub(super) fn sync_login_dialog(&mut self, server_before: Option<&str>) {
        let Some(server) = self
            .dialog
            .as_ref()
            .filter(|d| matches!(d.purpose, Purpose::LogIn(_)))
            .and_then(|d| d.fields.first())
            .map(|f| f.value.clone())
        else {
            return;
        };
        let known = login_server(&server);
        let origin_changed = server_before.and_then(login_server) != known;
        let guessed = self.service_of(known.as_ref());
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        if origin_changed {
            dialog.fields[2].value.clear();
            dialog.set_options_after_field(0, service_row(known.as_ref(), guessed));
        }
        // A pick holds while the other fields are typed in.
        let provider = dialog
            .choice
            .as_ref()
            .and_then(|c| ProviderKind::ALL.get(c.selected).copied())
            .unwrap_or(guessed);
        dialog.link = known.as_ref().map(|server| token_link(server, provider));
    }

    /// Saves the service picked for `server`'s host; one the name already
    /// tells is not kept, so the guess applies again.
    pub(super) fn remember_service(&mut self, server: &str, picked: Option<ProviderKind>) {
        let (Some((_, host)), Some(picked)) = (credential::server_of(server), picked) else {
            return;
        };
        let keep = (picked != PROVIDERS.guess(&host)).then(|| picked.key());
        if self.settings.services.get(&host).map(String::as_str) != keep {
            self.settings.set_service(&host, keep);
            self.save_settings();
        }
    }

    /// Asks which hosting service `host` is (its tag on the settings
    /// screen was clicked).
    pub(super) fn service_dialog(&mut self, host: String) -> Task<Message> {
        let current = PROVIDERS.for_host(&self.settings.services, &host);
        let options = ProviderKind::ALL
            .iter()
            .map(|p| Opt::new(provider_label(*p)))
            .collect();
        let dialog = Dialog::confirm(
            format!("Which service is {host}?"),
            vec![
                "gf guesses it from the name. It decides which page makes tokens \
                 and what the log-in asks for."
                    .into(),
            ],
            "Save",
            Purpose::SetService {
                host,
                service: None,
            },
        )
        .options_in_a_row("service", options, service_index(current));
        self.open_dialog(dialog)
    }

    pub(super) fn set_service(
        &mut self,
        host: &str,
        service: Option<ProviderKind>,
    ) -> Task<Message> {
        if let Some(service) = service {
            self.remember_service(host, Some(service));
            self.flash(
                NoteKind::Info,
                format!("{host} is {}", provider_label(service)),
            );
        }
        Task::none()
    }

    pub(super) fn open_url(&mut self, url: &str) -> Task<Message> {
        if let Err(e) = crate::platform::open_url(url) {
            self.flash(NoteKind::Error, format!("could not open {url}: {e}"));
        }
        Task::none()
    }

    /// An operation failed because `error`'s server wants a login: ask
    /// for one, then run `retry` (the same operation) again, if given.
    pub(super) fn ask_login_for(
        &mut self,
        label: &str,
        error: &str,
        retry: Option<Retry>,
    ) -> Task<Message> {
        // Never replaces a question already asked.
        if self.dialog.is_some() || self.code_dialog.is_some() || self.rebase_editor().is_some() {
            return Task::none();
        }
        if self.settings_page.changing_login {
            return self.show_failure(label, error);
        }
        let server = crate::git_actions::auth_server(error);
        let reason = match &server {
            Some(s) => format!("{} failed: {s} wants a login.", label),
            None => format!("{label} failed: the server wants a login."),
        };
        self.login_dialog(LoginAsk {
            server,
            reason: Some(reason),
            retry: retry.map(Box::new),
            ..LoginAsk::default()
        })
    }

    /// HTTP(S) servers of the open tabs' remotes, the active tab's first.
    fn remote_servers(&self) -> Vec<Suggestion> {
        let mut found: Vec<Suggestion> = Vec::new();
        let tabs = self
            .active_tab()
            .into_iter()
            .chain(self.tabs.iter())
            .enumerate();
        for (i, tab) in tabs {
            let Some(snapshot) = tab.snapshot.as_ref() else {
                continue;
            };
            for remote in &snapshot.remotes {
                let urls = [Some(remote.fetch_url.as_str()), remote.push_url.as_deref()];
                // Only real URLs: a bare word in a remote is a local path.
                let servers = urls.into_iter().flatten().filter_map(login_server);
                for (protocol, host) in servers {
                    let value = format!("{protocol}://{host}");
                    if found.iter().any(|s| s.value == value) {
                        continue;
                    }
                    let hint = if i == 0 {
                        format!("this tab · {}", remote.name)
                    } else {
                        format!("{} · {}", tab.name(), remote.name)
                    };
                    found.push(Suggestion::new(value, hint));
                }
            }
        }
        found
    }

    /// The servers the open tabs' remotes point at, the active tab's
    /// first: HTTP(S) ones and SSH ones.
    fn servers_in_use(&self) -> (Vec<ServerUse>, Vec<ServerUse>) {
        let mut http: Vec<ServerUse> = Vec::new();
        let mut ssh: Vec<ServerUse> = Vec::new();
        let active = self.active_tab().map(|tab| tab.repo.cwd().to_path_buf());
        let tabs = self.active_tab().into_iter().chain(
            self.tabs
                .iter()
                .filter(|tab| Some(tab.repo.cwd()) != active.as_deref()),
        );
        for tab in tabs {
            let Some(snapshot) = tab.snapshot.as_ref() else {
                continue;
            };
            let common_dir = tab.repo.common_dir();
            let name = if common_dir.ends_with(".git") {
                common_dir
                    .parent()
                    .and_then(std::path::Path::file_name)
                    .map_or_else(|| tab.name(), |name| name.to_string_lossy().into_owned())
            } else {
                tab.name()
            };
            for remote in &snapshot.remotes {
                let urls = [Some(remote.fetch_url.as_str()), remote.push_url.as_deref()];
                for url in urls.into_iter().flatten() {
                    let (list, protocol, host) = if url.contains("://")
                        && let Some((protocol, host)) = credential::server_of(url)
                    {
                        (&mut http, protocol, host)
                    } else if let Some(host) = ssh_host(url) {
                        (&mut ssh, "ssh".to_owned(), host)
                    } else {
                        continue;
                    };
                    let index = match list
                        .iter()
                        .position(|s| s.protocol == protocol && s.host == host)
                    {
                        Some(index) => index,
                        None => {
                            list.push(ServerUse {
                                protocol,
                                host,
                                url: without_user(url),
                                repositories: Default::default(),
                            });
                            list.len() - 1
                        }
                    };
                    list[index].repositories.add(common_dir, &name);
                }
            }
        }
        (http, ssh)
    }

    pub(super) fn log_in(&mut self, values: &[String], retry: Option<Box<Retry>>) -> Task<Message> {
        let Some((protocol, host)) = values.first().and_then(|v| login_server(v)) else {
            return Task::none();
        };
        let value = |i: usize| values.get(i).cloned().unwrap_or_default();
        let account = Account {
            protocol: protocol.clone(),
            host: host.clone(),
            username: value(1),
        };
        // Checked against a repository of the open tabs on that server;
        // an operation to run again checks it by itself.
        let check_url = retry
            .is_none()
            .then(|| {
                self.servers_in_use()
                    .0
                    .into_iter()
                    .find(|s| s.protocol == protocol && s.host == host)
                    .map(|s| s.url)
            })
            .flatten();
        let login = Login {
            protocol,
            host,
            username: value(1),
            token: value(2),
        };
        let runner = self.runner.clone();
        self.settings_page.changing_login = true;
        Task::perform(
            async move { save_and_check(&runner, &login, check_url).await },
            move |result| Message::LoggedIn(account.clone(), result, retry.clone()),
        )
    }

    pub(super) fn logged_in(
        &mut self,
        account: Account,
        result: Result<LoginCheck, String>,
        retry: Option<Box<Retry>>,
    ) -> Task<Message> {
        self.settings_page.changing_login = false;
        match result {
            Ok(check) => {
                let (kind, text) = match &check {
                    LoginCheck::Verified => (
                        NoteKind::Info,
                        format!(
                            "Login verified for {} as {}.",
                            account.host, account.username
                        ),
                    ),
                    LoginCheck::Reachable => (
                        NoteKind::Info,
                        format!(
                            "Saved login for {} as {}. Repository reachable.",
                            account.host, account.username
                        ),
                    ),
                    LoginCheck::NotChecked => (
                        NoteKind::Info,
                        format!(
                            "Saved login for {} as {}. Repository access unchecked.",
                            account.host, account.username
                        ),
                    ),
                    LoginCheck::Unreachable(e) => (
                        NoteKind::Error,
                        format!(
                            "Saved login for {}. Could not confirm repository access: {e}",
                            account.host
                        ),
                    ),
                };
                self.flash(kind, text);
                self.settings.remember_account(account);
                self.save_settings();
                let refresh = if self.settings_open {
                    self.refresh_settings_page(false)
                } else {
                    Task::none()
                };
                let again = match retry {
                    Some(retry) => self.run_purpose(retry.purpose, retry.values, Vec::new()),
                    None => Task::none(),
                };
                Task::batch([refresh, again])
            }
            Err(error) => self.login_dialog(LoginAsk {
                updating: retry.is_none() && self.settings.accounts.contains(&account),
                server: Some(format!("{}://{}", account.protocol, account.host)),
                user: Some(account.username),
                error: Some(error),
                retry,
                ..LoginAsk::default()
            }),
        }
    }

    pub(super) fn log_out(&mut self, account: Account) -> Task<Message> {
        self.settings_page.changing_login = true;
        let runner = self.runner.clone();
        let (protocol, host, user) = (
            account.protocol.clone(),
            account.host.clone(),
            account.username.clone(),
        );
        Task::perform(
            async move {
                credential::forget_login(&runner, &protocol, &host, &user)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |result| Message::LoggedOut(account.clone(), result),
        )
    }

    pub(super) fn logged_out(
        &mut self,
        account: Account,
        result: Result<(), String>,
    ) -> Task<Message> {
        self.settings_page.changing_login = false;
        match result {
            Ok(()) => {
                self.settings.accounts.retain(|a| *a != account);
                self.settings_page.tokens.retain(|(a, _)| *a != account);
                self.save_settings();
                self.flash(
                    NoteKind::Info,
                    format!(
                        "Removed saved login for {} on {}",
                        account.username, account.host
                    ),
                );
                if self.settings_open {
                    return self.refresh_settings_page(false);
                }
            }
            Err(error) => self.flash(
                NoteKind::Error,
                format!(
                    "Could not remove the saved login for {} on {}: {error}",
                    account.username, account.host
                ),
            ),
        }
        Task::none()
    }

    pub(super) fn cycle_theme(&mut self, backwards: bool) {
        let presets = theme::PRESET_NAMES;
        let current = presets.iter().position(|name| *name == self.tokens.name);
        let step = if backwards { presets.len() - 1 } else { 1 };
        let next = current.map_or(0, |index| (index + step) % presets.len());
        self.set_theme(presets[next]);
    }

    pub(super) fn set_theme(&mut self, name: &str) {
        self.tokens = theme::preset(name).with_font_size(self.settings.appearance.font_size);
        self.settings.appearance.theme = name.to_owned();
        self.flash(NoteKind::Info, format!("theme: {name}"));
        self.save_settings();
    }
}

async fn probe_account(
    runner: &gf_git::GitRunner,
    account: &Account,
    url: Option<&str>,
) -> gf_core::Result<AccountStatus> {
    let Some(login) =
        credential::saved_login(runner, &account.protocol, &account.host, &account.username)
            .await?
    else {
        return Ok(AccountStatus::Missing);
    };
    let Some(url) = url else {
        return Ok(AccountStatus::Saved);
    };
    match credential::check_remote_access(runner, url, &login).await {
        Ok(true) => Ok(AccountStatus::Verified),
        Ok(false) => Ok(AccountStatus::Saved),
        Err(error) if crate::git_actions::needs_login(&error.to_string()) => {
            Ok(AccountStatus::Rejected)
        }
        Err(error) => Err(error),
    }
}

/// Checks the candidate before replacing anything in Git's credential helper.
async fn save_and_check(
    runner: &gf_git::GitRunner,
    login: &Login,
    check_url: Option<String>,
) -> Result<LoginCheck, String> {
    let check = match check_url {
        None => LoginCheck::NotChecked,
        Some(url) => match credential::check_remote_access(runner, &url, login).await {
            Ok(true) => LoginCheck::Verified,
            Ok(false) => LoginCheck::Reachable,
            Err(e) => {
                let e = e.to_string();
                if crate::git_actions::needs_login(&e) {
                    return Err(format!(
                        "{} refused this user name and token. Check both and try again.",
                        login.host
                    ));
                }
                LoginCheck::Unreachable(first_line(&e))
            }
        },
    };
    credential::store_login(runner, login)
        .await
        .map_err(|e| e.to_string())?;
    Ok(check)
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or_default()
        .to_owned()
}

/// `https://me@host/x` → `https://host/x`: git asks the helper for the
/// saved login instead of for `me`'s.
fn without_user(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let authority_end = rest.find('/').unwrap_or(rest.len());
            match rest[..authority_end].rsplit_once('@') {
                Some((_, host)) => format!("{scheme}://{host}{}", &rest[authority_end..]),
                None => url.to_owned(),
            }
        }
        None => url.to_owned(),
    }
}

/// The host of an SSH remote: `git@github.com:a/b.git`,
/// `ssh://git@host:22/a.git`. `None` for local paths and other URLs.
fn ssh_host(url: &str) -> Option<String> {
    let url = url.trim();
    let host = if let Some(rest) = url.strip_prefix("ssh://") {
        let authority = rest.split('/').next().unwrap_or_default();
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        host.split(':').next().unwrap_or_default()
    } else if url.contains("://") {
        return None;
    } else {
        // scp-like `user@host:path`; `C:\x` and `./a:b` are paths.
        let (before, _) = url.split_once(':')?;
        if before.contains(['/', '\\']) || before.len() < 2 {
            return None;
        }
        before.rsplit_once('@').map_or(before, |(_, h)| h)
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// What the settings screen read in the background.
#[derive(Debug, Clone)]
pub enum Probed {
    Helper(u64, Probe<Option<String>>),
    Token(u64, Account, Probe<AccountStatus>),
    /// All probes from an explicit refresh have completed.
    Refreshed(u64),
    /// `credential.helper` was set (to the value), or not.
    HelperSet(Result<String, String>),
}

/// The result of trying to read a remote before saving a login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginCheck {
    /// Git approved the token after an authenticated read.
    Verified,
    /// A remote returned its refs. It might be public.
    Reachable,
    /// No open repository is on that server, or the retried operation will check access.
    NotChecked,
    /// The server could not be reached (the error's last line).
    Unreachable(String),
}

/// A TOOLS setting (Settings → TOOLS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Editor,
    Terminal,
    CloneRoot,
    WorktreeRoot,
}

/// What a TOOLS value lacks, if anything.
pub(super) fn check_tool(tool: Tool, value: &str) -> Option<String> {
    match tool {
        Tool::Editor if value.trim().is_empty() => Some("an editor command is required".into()),
        Tool::CloneRoot
            if !value.trim().is_empty()
                && !crate::widgets::format::expand_tilde(value.trim()).is_absolute() =>
        {
            Some("enter an absolute path or a path starting with ~/".into())
        }
        _ => None,
    }
}

/// Presets for [`App::login_dialog`].
#[derive(Debug, Default)]
pub(super) struct LoginAsk {
    pub server: Option<String>,
    pub user: Option<String>,
    /// An existing account's token is being replaced.
    pub updating: bool,
    /// Why it is asked (said first).
    pub reason: Option<String>,
    /// Why the last attempt failed.
    pub error: Option<String>,
    pub retry: Option<Box<Retry>>,
}

/// What to run again once logged in: a dialog's action with its values.
#[derive(Debug, Clone)]
pub struct Retry {
    /// Said on the button: `push main` → "Log in and push main".
    pub label: String,
    pub purpose: Purpose,
    pub values: Vec<String>,
}

/// How to run an operation the server refused again after logging in;
/// `None` for one that cannot simply run again (deleting branches).
pub(super) fn retry_after_login(kind: &OpKind, label: &str, repo: RepoId) -> Option<Retry> {
    let (purpose, values) = match kind.clone() {
        OpKind::Fetch => (Purpose::Fetch, Vec::new()),
        OpKind::Pull(mode) => (Purpose::Pull { repo, mode }, Vec::new()),
        OpKind::Push {
            branch: Some(branch),
            to: Some(to),
            offer_force: true,
            tags,
        } if label.starts_with("publish ") => (Purpose::Publish { branch, tags }, vec![to.remote]),
        OpKind::Push {
            branch: Some(branch),
            to,
            offer_force: true,
            tags,
        } => (
            Purpose::Push {
                repo,
                branch,
                to,
                tags,
                allow_configured_rewrites: false,
            },
            Vec::new(),
        ),
        OpKind::Remote(RemoteOp::PushTag { remote, name }) => {
            (Purpose::PushTag { remote, name }, Vec::new())
        }
        OpKind::Remote(RemoteOp::UpdateSubmodules) => (Purpose::UpdateSubmodules, Vec::new()),
        OpKind::Remote(RemoteOp::LfsPull) => (Purpose::LfsPull, Vec::new()),
        // Already confirmed to overwrite: the same force push again.
        OpKind::Push {
            branch,
            to,
            offer_force: false,
            ..
        } => (Purpose::ForcePush { branch, to }, Vec::new()),
        _ => return None,
    };
    Some(Retry {
        label: label.to_owned(),
        purpose,
        values,
    })
}

/// Compact server label for notices in settings.
fn server_label(protocol: &str, host: &str) -> String {
    if protocol == "https" {
        host.to_owned()
    } else {
        format!("{protocol}://{host}")
    }
}

/// Where `provider` sits among the log-in dialog's options.
fn service_index(provider: ProviderKind) -> usize {
    ProviderKind::ALL
        .iter()
        .position(|p| *p == provider)
        .unwrap_or(ProviderKind::ALL.len() - 1)
}

/// The log-in dialog's service row, asked only for a server whose name
/// tells nothing (a self-hosted `git.example.com`); `github.com` and the
/// like need no pick. `provider` is the one selected.
fn service_row(
    server: Option<&(String, String)>,
    provider: ProviderKind,
) -> Option<(String, Vec<Opt>, usize)> {
    server
        .filter(|(_, host)| PROVIDERS.guess(host) == ProviderKind::Other)
        .map(|_| {
            let options = ProviderKind::ALL
                .iter()
                .map(|p| Opt::new(provider_label(*p)))
                .collect();
            ("Service".to_owned(), options, service_index(provider))
        })
}

/// The log-in dialog's token line: what to make and, for a known service,
/// a button to the page that makes it. It names the service, as the row
/// to pick one is left out where the name tells it.
fn token_link((protocol, host): &(String, String), provider: ProviderKind) -> Link {
    let Some(setup) = PROVIDERS.token_setup(provider, protocol, host) else {
        return Link {
            text: "Enter an access token or server password.".to_owned(),
            open: None,
        };
    };
    Link {
        text: setup.guidance.to_owned(),
        open: setup.page.map(|page| ("Get token ↗".to_owned(), page)),
    }
}

/// Login fields require an explicit HTTP(S) URL, including for private hosts.
fn login_server(input: &str) -> Option<(String, String)> {
    input.split_once("://")?;
    credential::server_of(input)
}

/// What the log-in dialog's fields lack, if anything (server, user name,
/// token).
pub(super) fn check_login(values: &[String]) -> Option<String> {
    let value = |i: usize| values.get(i).map(String::as_str).unwrap_or_default();
    if login_server(value(0)).is_none() {
        Some("Enter a server URL like https://github.com. SSH remotes use SSH keys.".into())
    } else if value(1).trim().is_empty() {
        Some("a user name is required".into())
    } else if value(2).trim().is_empty() {
        Some("a token is required".into())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(v: [&str; 3]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_login_needs_an_https_server_a_user_and_a_token() {
        assert_eq!(
            check_login(&values(["https://github.com", "me", "tok"])),
            None
        );
        assert_eq!(
            check_login(&values(["http://gitlab.local:8080", "me", "tok"])),
            None
        );
        assert_eq!(
            check_login(&values(["https://gitlab.example.com/a/b.git", "me", "tok"])),
            None
        );
        assert!(check_login(&values(["", "me", "tok"])).is_some());
        assert!(
            check_login(&values(["github.com", "me", "tok"]))
                .is_some_and(|e| e.contains("https://github.com"))
        );
        assert!(
            check_login(&values(["git@github.com:a/b.git", "me", "tok"]))
                .is_some_and(|e| e.contains("SSH"))
        );
        assert!(check_login(&values(["https://github.com", "", "tok"])).is_some());
        assert!(check_login(&values(["https://github.com", "me", ""])).is_some());
    }

    #[test]
    fn refused_syncs_run_again_as_they_were() {
        let repo = RepoId::default();
        let to = super::super::PushTo {
            remote: "origin".into(),
            refspec: "topic:topic".into(),
        };
        let push = |to: Option<super::super::PushTo>, offer_force| OpKind::Push {
            branch: Some("topic".into()),
            to,
            offer_force,
            tags: None,
        };
        let retry = |kind: &OpKind, label: &str| retry_after_login(kind, label, repo);

        let r = retry(&push(Some(to.clone()), true), "publish topic").expect("publish");
        assert!(matches!(r.purpose, Purpose::Publish { .. }));
        assert_eq!(r.values, ["origin"], "to the same remote");
        let r = retry(&push(None, true), "push topic").expect("push");
        assert!(matches!(
            r.purpose,
            Purpose::Push {
                to: None,
                allow_configured_rewrites: false,
                ..
            }
        ));
        let r = retry(&push(Some(to), false), "force push (with lease)").expect("force");
        assert!(matches!(r.purpose, Purpose::ForcePush { to: Some(_), .. }));
        assert!(matches!(
            retry(&OpKind::Fetch, "fetch").map(|r| r.purpose),
            Some(Purpose::Fetch)
        ));
        assert!(matches!(
            retry(&OpKind::Pull(gf_git::PullMode::Rebase), "pull").map(|r| r.purpose),
            Some(Purpose::Pull {
                mode: gf_git::PullMode::Rebase,
                ..
            })
        ));
        let tag = OpKind::Remote(RemoteOp::PushTag {
            remote: "origin".into(),
            name: "v1".into(),
        });
        assert!(matches!(
            retry(&tag, "push tag v1").map(|r| r.purpose),
            Some(Purpose::PushTag { .. })
        ));
        assert!(retry(&OpKind::Commit, "commit").is_none());
        assert!(retry(&OpKind::DeleteBranches("/g".into()), "delete").is_none());
    }

    #[test]
    fn http_servers_keep_their_scheme_in_the_dialog() {
        assert_eq!(server_label("https", "github.com"), "github.com");
        assert_eq!(server_label("http", "10.0.0.5"), "http://10.0.0.5");
    }
}
