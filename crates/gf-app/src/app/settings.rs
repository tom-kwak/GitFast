//! Routes the settings screen and prompt to the app's shared preferences,
//! and logs in to / out of git servers (ACCOUNTS).

use gf_config::Account;
use gf_git::ops::credential::{self, Login};
use iced::advanced::widget::operation::focusable;
use iced::{Element, Task};

use gf_core::model::RepoId;

use super::dialogs::{Confirm, Purpose};
use super::{App, Message, OpKind, RemoteOp};
use crate::features::command_log::NoteKind;
use crate::features::dialog::{Dialog, Link, Opt, Suggestion};
use crate::features::settings_view::{self, Probe, Provider, Section, ServerUse};
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
                self.refresh_settings_page(),
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
    pub(super) fn refresh_settings_page(&mut self) -> Task<Message> {
        let (servers, ssh) = self.servers_in_use();
        let page = &mut self.settings_page;
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
            .map_or(("https".to_owned(), "github.com".to_owned()), |a| {
                (a.protocol.clone(), a.host.clone())
            });
        let runner = self.runner.clone();
        let helper = Task::perform(
            async move {
                match credential::credential_helper(&runner, &protocol, &host).await {
                    Ok(helper) => Probe::Known(helper),
                    Err(e) => Probe::Failed(e.to_string()),
                }
            },
            |probe| Message::SettingsProbed(Probed::Helper(probe)),
        );
        let tokens = self.settings.accounts.iter().cloned().map(|account| {
            let runner = self.runner.clone();
            Task::perform(
                async move {
                    let probe = match credential::has_login(
                        &runner,
                        &account.protocol,
                        &account.host,
                        &account.username,
                    )
                    .await
                    {
                        Ok(saved) => Probe::Known(saved),
                        Err(e) => Probe::Failed(e.to_string()),
                    };
                    (account, probe)
                },
                |(account, probe)| Message::SettingsProbed(Probed::Token(account, probe)),
            )
        });
        Task::batch(std::iter::once(helper).chain(tokens))
    }

    pub(super) fn settings_probed(&mut self, probed: Probed) {
        let page = &mut self.settings_page;
        match probed {
            Probed::Helper(probe) => page.helper = probe,
            Probed::HelperSet(Ok(helper)) => {
                page.helper = Probe::Known(Some(helper.clone()));
                self.flash(
                    NoteKind::Info,
                    format!("git keeps logins with `{helper}` now"),
                );
            }
            Probed::HelperSet(Err(e)) => self.flash(
                NoteKind::Error,
                format!("could not set git's credential helper: {e}"),
            ),
            Probed::Token(account, probe) => {
                match page.tokens.iter_mut().find(|(a, _)| *a == account) {
                    Some(entry) => entry.1 = probe,
                    None => page.tokens.push((account, probe)),
                }
            }
        }
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
            M::AddAccount => self.login_dialog(LoginAsk::default()),
            M::LogInTo(server) => self.login_dialog(LoginAsk {
                server: Some(server),
                ..LoginAsk::default()
            }),
            M::UpdateToken(i) => match self.settings.accounts.get(i).cloned() {
                Some(account) => self.login_dialog(LoginAsk {
                    server: Some(server_label(&account.protocol, &account.host)),
                    user: Some(account.username),
                    ..LoginAsk::default()
                }),
                None => Task::none(),
            },
            M::LogOut(i) => match self.settings.accounts.get(i).cloned() {
                Some(account) => self.confirm(Confirm::delete(
                    format!("Log out of {}?", account.host),
                    vec![
                        format!(
                            "git's credential helper forgets the token of {} on {}.",
                            account.username, account.host
                        ),
                        "Fetching and pushing over HTTPS there will fail until you log in again."
                            .into(),
                    ],
                    "Log out",
                    Purpose::LogOut(account),
                )),
                None => Task::none(),
            },
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
            M::EditWorktreeRoot => self.tool_dialog(Tool::WorktreeRoot),
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
        let mut servers = self.remote_servers();
        // The active tab's server, unless already logged in there.
        let server = ask.server.clone().unwrap_or_else(|| {
            servers
                .iter()
                .find(|s| s.hint.starts_with("this tab") && !s.disabled)
                .map(|s| s.value.clone())
                .unwrap_or_default()
        });
        for host in WELL_KNOWN {
            if servers.iter().any(|s| s.value == host) {
                continue;
            }
            let account = self
                .settings
                .accounts
                .iter()
                .find(|a| a.protocol == "https" && a.host == host);
            let hint = account.map_or_else(
                || Provider::of(host).name().to_owned(),
                |a| format!("logged in as {}", a.username),
            );
            servers.push(Suggestion::new(host, hint).disabled(account.is_some()));
        }
        let known = credential::server_of(&server);
        let user = ask.user.clone().unwrap_or_else(|| {
            known
                .as_ref()
                .and_then(|(protocol, host)| {
                    self.settings
                        .accounts
                        .iter()
                        .find(|a| a.protocol == *protocol && a.host == *host)
                })
                .map(|a| a.username.clone())
                .unwrap_or_default()
        });
        let mut body = ask.reason.clone().map_or_else(Vec::new, |r| vec![r]);
        let provider = self.service_of(known.as_ref());
        body.push(
            "gf hands it to git's credential helper (Keychain, Credential Manager …) and \
             does not keep it; git in the terminal uses it too."
                .to_owned(),
        );
        let confirm = match &ask.retry {
            Some(retry) => format!(
                "Log in and {}",
                crate::widgets::format::truncate(&retry.label, 40)
            ),
            None => "Log in".to_owned(),
        };
        // The first field still empty takes the typing.
        let first_empty = [server.is_empty(), user.is_empty()]
            .iter()
            .position(|empty| *empty)
            .unwrap_or(2);
        let dialog = Dialog::confirm(
            "Log in to a git server",
            body,
            confirm,
            Purpose::LogIn(ask.retry.clone()),
        )
        .field("server", server, "github.com")
        .suggest(servers, "not a server of the open tabs");
        let options = Provider::ALL
            .iter()
            .map(|p| Opt::new(p.pick_label()))
            .collect();
        let selected = service_index(provider);
        let mut dialog = dialog
            .field("user name", user, "your user name on the server")
            .field("token", "", "personal access token or password")
            .secret()
            .options_in_a_row("service", options, selected)
            .link(token_link(known.as_ref(), provider));
        dialog.error = ask.error;
        let task = self.open_dialog(dialog);
        match first_empty {
            0 => task,
            i => crate::features::dialog::field_id(i).map_or(task, iced::widget::operation::focus),
        }
    }

    /// The hosting service of a server: picked for its host in the
    /// settings, else guessed from the name. `Other` for no server.
    fn service_of(&self, server: Option<&(String, String)>) -> Provider {
        server.map_or(Provider::Other, |(_, host)| {
            Provider::for_host(&self.settings.services, host)
        })
    }

    /// Keeps the log-in dialog in step after one of its messages: a
    /// server typed in picks its service again, and the token line
    /// follows the server and the service. `server_before` is the server
    /// field's value before the message.
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
        let known = credential::server_of(&server);
        let guess = (server_before != Some(server.as_str()))
            .then(|| service_index(self.service_of(known.as_ref())));
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        let Some(choice) = dialog.choice.as_mut() else {
            return;
        };
        if let Some(i) = guess {
            choice.selected = i;
        }
        let provider = Provider::ALL
            .get(choice.selected)
            .copied()
            .unwrap_or(Provider::Other);
        dialog.link = Some(token_link(known.as_ref(), provider));
    }

    /// Saves the service picked for `server`'s host; one the name already
    /// tells is not kept, so the guess applies again.
    pub(super) fn remember_service(&mut self, server: &str, picked: Option<Provider>) {
        let (Some((_, host)), Some(picked)) = (credential::server_of(server), picked) else {
            return;
        };
        let keep = (picked != Provider::of(&host)).then(|| picked.key());
        if self.settings.services.get(&host).map(String::as_str) != keep {
            self.settings.set_service(&host, keep);
            self.save_settings();
        }
    }

    /// Asks which hosting service `host` is (its tag on the settings
    /// screen was clicked).
    pub(super) fn service_dialog(&mut self, host: String) -> Task<Message> {
        let current = Provider::for_host(&self.settings.services, &host);
        let options = Provider::ALL
            .iter()
            .map(|p| Opt::new(p.pick_label()))
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

    pub(super) fn set_service(&mut self, host: &str, service: Option<Provider>) -> Task<Message> {
        if let Some(service) = service {
            self.remember_service(host, Some(service));
            self.flash(
                NoteKind::Info,
                format!("{host} is {}", service.pick_label()),
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

    /// HTTPS servers of the open tabs' remotes, the active tab's first.
    /// One already logged in to is shown, but not offered.
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
                let servers = urls
                    .into_iter()
                    .flatten()
                    .filter(|url| url.contains("://"))
                    .filter_map(credential::server_of);
                for (protocol, host) in servers {
                    let value = server_label(&protocol, &host);
                    if found.iter().any(|s| s.value == value) {
                        continue;
                    }
                    let logged_in = self
                        .settings
                        .accounts
                        .iter()
                        .find(|a| a.protocol == protocol && a.host == host);
                    let hint = match logged_in {
                        Some(a) => format!("logged in as {}", a.username),
                        None if i == 0 => format!("this tab · {}", remote.name),
                        None => format!("{} · {}", tab.name(), remote.name),
                    };
                    found.push(Suggestion::new(value, hint).disabled(logged_in.is_some()));
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
            for remote in &snapshot.remotes {
                let used_by = format!("{} · {}", tab.name(), remote.name);
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
                    match list
                        .iter_mut()
                        .find(|s| s.protocol == protocol && s.host == host)
                    {
                        Some(server) if !server.used_by.contains(&used_by) => {
                            server.used_by.push(used_by.clone());
                        }
                        Some(_) => {}
                        None => list.push(ServerUse {
                            protocol,
                            host,
                            url: without_user(url),
                            used_by: vec![used_by.clone()],
                        }),
                    }
                }
            }
        }
        (http, ssh)
    }

    pub(super) fn log_in(&mut self, values: &[String], retry: Option<Box<Retry>>) -> Task<Message> {
        let Some((protocol, host)) = values.first().and_then(|v| credential::server_of(v)) else {
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
        Task::perform(
            async move { save_and_check(&runner, &login, check_url).await },
            move |result| Message::LoggedIn(account.clone(), result, retry.clone()),
        )
    }

    pub(super) fn logged_in(
        &mut self,
        account: Account,
        result: Result<LoginSaved, String>,
        retry: Option<Box<Retry>>,
    ) -> Task<Message> {
        match result {
            Ok(saved) => {
                let (kind, text) = match &saved.check {
                    LoginCheck::Verified => (
                        NoteKind::Info,
                        format!(
                            "✓ logged in to {} as {} — the server took the token",
                            account.host, account.username
                        ),
                    ),
                    LoginCheck::NotChecked => (
                        NoteKind::Info,
                        format!(
                            "logged in to {} as {} — kept by git's `{}` helper",
                            account.host, account.username, saved.helper
                        ),
                    ),
                    LoginCheck::Unreachable(e) => (
                        NoteKind::Error,
                        format!(
                            "logged in to {}, but could not reach it to check the token: {e}",
                            account.host
                        ),
                    ),
                };
                self.flash(kind, text);
                self.settings.remember_account(account);
                self.save_settings();
                let refresh = if self.settings_open {
                    self.refresh_settings_page()
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
                server: Some(server_label(&account.protocol, &account.host)),
                user: Some(account.username),
                error: Some(error),
                retry,
                ..LoginAsk::default()
            }),
        }
    }

    pub(super) fn log_out(&mut self, account: Account) -> Task<Message> {
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
        match result {
            Ok(()) => {
                self.settings.accounts.retain(|a| *a != account);
                self.settings_page.tokens.retain(|(a, _)| *a != account);
                self.save_settings();
                self.flash(NoteKind::Info, format!("logged out of {}", account.host));
            }
            Err(error) => self.flash(
                NoteKind::Error,
                format!("could not log out of {}: {error}", account.host),
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

/// Saves the login with git's credential helper, then checks it against
/// `check_url` if given. A token the server refuses is not kept.
async fn save_and_check(
    runner: &gf_git::GitRunner,
    login: &Login,
    check_url: Option<String>,
) -> Result<LoginSaved, String> {
    let helper = credential::store_login(runner, login)
        .await
        .map_err(|e| e.to_string())?;
    let check = match check_url {
        None => LoginCheck::NotChecked,
        Some(url) => match credential::verify_login(runner, &url).await {
            Ok(()) => LoginCheck::Verified,
            Err(e) => {
                let e = e.to_string();
                if crate::git_actions::needs_login(&e) {
                    // git usually erases it already; make sure.
                    let _ = credential::forget_login(
                        runner,
                        &login.protocol,
                        &login.host,
                        &login.username,
                    )
                    .await;
                    return Err(format!(
                        "{} refused this user name and token. Check both and try again.",
                        login.host
                    ));
                }
                LoginCheck::Unreachable(first_line(&e))
            }
        },
    };
    Ok(LoginSaved { helper, check })
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
    Helper(Probe<Option<String>>),
    Token(Account, Probe<bool>),
    /// `credential.helper` was set (to the value), or not.
    HelperSet(Result<String, String>),
}

/// A login git's credential helper took.
#[derive(Debug, Clone)]
pub struct LoginSaved {
    /// The helper's name.
    pub helper: String,
    pub check: LoginCheck,
}

/// Whether the server took a login when gf tried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginCheck {
    /// It listed a repository's branches with it.
    Verified,
    /// No open repository is on that server (or an operation to run again
    /// will try it).
    NotChecked,
    /// The server could not be reached (the error's last line).
    Unreachable(String),
}

/// A TOOLS setting (Settings → TOOLS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Editor,
    Terminal,
    WorktreeRoot,
}

/// What a TOOLS value lacks, if anything.
pub(super) fn check_tool(tool: Tool, value: &str) -> Option<String> {
    match tool {
        Tool::Editor if value.trim().is_empty() => Some("an editor command is required".into()),
        _ => None,
    }
}

/// Presets for [`App::login_dialog`].
#[derive(Debug, Default)]
pub(super) struct LoginAsk {
    pub server: Option<String>,
    pub user: Option<String>,
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

/// How a server reads in the dialog: `https` is left out.
fn server_label(protocol: &str, host: &str) -> String {
    if protocol == "https" {
        host.to_owned()
    } else {
        format!("{protocol}://{host}")
    }
}

/// Where `provider` sits among the log-in dialog's options.
fn service_index(provider: Provider) -> usize {
    Provider::ALL
        .iter()
        .position(|p| *p == provider)
        .unwrap_or(Provider::ALL.len() - 1)
}

/// The log-in dialog's token line: what to make and, for a known service,
/// a button to the page that makes it.
fn token_link(server: Option<&(String, String)>, provider: Provider) -> Link {
    let page = server.and_then(|(protocol, host)| provider.token_page(protocol, host));
    Link {
        text: format!("Use {} as the password.", provider.token_hint()),
        open: page.map(|page| ("Get token ↗".to_owned(), page)),
    }
}

/// What the log-in dialog's fields lack, if anything (server, user name,
/// token).
pub(super) fn check_login(values: &[String]) -> Option<String> {
    let value = |i: usize| values.get(i).map(String::as_str).unwrap_or_default();
    if credential::server_of(value(0)).is_none() {
        Some(if value(0).is_empty() {
            "enter a server, e.g. github.com".into()
        } else {
            format!(
                "`{}` is not an HTTPS server (SSH remotes use your SSH keys)",
                value(0)
            )
        })
    } else if value(1).is_empty() {
        Some("a user name is required".into())
    } else if value(2).is_empty() {
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
        assert_eq!(check_login(&values(["github.com", "me", "tok"])), None);
        assert_eq!(
            check_login(&values(["https://gitlab.example.com/a/b.git", "me", "tok"])),
            None
        );
        assert!(check_login(&values(["", "me", "tok"])).is_some());
        assert!(
            check_login(&values(["git@github.com:a/b.git", "me", "tok"]))
                .is_some_and(|e| e.contains("SSH"))
        );
        assert!(check_login(&values(["github.com", "", "tok"])).is_some());
        assert!(check_login(&values(["github.com", "me", ""])).is_some());
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
