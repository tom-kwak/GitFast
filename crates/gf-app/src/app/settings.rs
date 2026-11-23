//! Routes the settings view and prompt to the app's shared preferences,
//! and logs in to / out of git servers (ACCOUNTS).

use gf_config::Account;
use gf_git::ops::credential::{self, Login};
use iced::Task;
use iced::advanced::widget::operation::focusable;

use gf_core::model::RepoId;

use super::dialogs::{Confirm, Purpose};
use super::{App, Message, OpKind, RemoteOp};
use crate::features::dialog::{Dialog, Suggestion};
use crate::features::{command_log::NoteKind, settings_view};
use crate::theme;

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
            iced::advanced::widget::operate(focusable::unfocus())
        } else {
            self.restore_focus()
        }
    }

    pub(super) fn on_settings(&mut self, message: settings_view::Message) -> Task<Message> {
        if !self.settings_open || self.dialog.is_some() {
            return Task::none();
        }
        match message {
            settings_view::Message::SetTheme(name) => {
                self.set_theme(name);
                Task::none()
            }
            settings_view::Message::LogIn => self.login_dialog(LoginAsk::default()),
            settings_view::Message::LogOut(i) => match self.settings.accounts.get(i).cloned() {
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
            settings_view::Message::Close => self.toggle_settings(),
        }
    }

    /// The log-in dialog: server (offered from the open tabs' HTTPS
    /// remotes), user name, token. `ask` presets it: after a failed
    /// attempt, or for a push/pull/fetch the server refused (then run
    /// again once logged in).
    pub(super) fn login_dialog(&mut self, ask: LoginAsk) -> Task<Message> {
        let servers = self.remote_servers();
        // The active tab's server, unless already logged in there.
        let server = ask.server.clone().unwrap_or_else(|| {
            servers
                .iter()
                .find(|s| s.hint.starts_with("this tab") && !s.disabled)
                .map(|s| s.value.clone())
                .unwrap_or_default()
        });
        let user = ask.user.clone().unwrap_or_else(|| {
            credential::server_of(&server)
                .and_then(|(protocol, host)| {
                    self.settings
                        .accounts
                        .iter()
                        .find(|a| a.protocol == protocol && a.host == host)
                })
                .map(|a| a.username.clone())
                .unwrap_or_default()
        });
        let mut body = ask.reason.clone().map_or_else(Vec::new, |r| vec![r]);
        body.extend([
            "Use a personal access token as the password \
             (GitHub: Settings → Developer settings; GitLab: Preferences → Access tokens)."
                .to_owned(),
            "gf hands it to git's credential helper (Keychain, Credential Manager …) and \
             does not keep it; git in the terminal uses it too."
                .to_owned(),
        ]);
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
        let mut dialog = Dialog::confirm(
            "Log in to a git server",
            body,
            confirm,
            Purpose::LogIn(ask.retry.clone()),
        )
        .field("server", server, "github.com");
        if !servers.is_empty() {
            dialog = dialog.suggest(servers, "no remote of the open tabs is on it");
        }
        let mut dialog = dialog
            .field("user name", user, "your user name on the server")
            .field("token", "", "personal access token or password")
            .secret();
        dialog.error = ask.error;
        let task = self.open_dialog(dialog);
        match first_empty {
            0 => task,
            i => crate::features::dialog::field_id(i).map_or(task, iced::widget::operation::focus),
        }
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
        let login = Login {
            protocol,
            host,
            username: value(1),
            token: value(2),
        };
        let runner = self.runner.clone();
        Task::perform(
            async move {
                credential::store_login(&runner, &login)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |result| Message::LoggedIn(account.clone(), result, retry.clone()),
        )
    }

    pub(super) fn logged_in(
        &mut self,
        account: Account,
        result: Result<String, String>,
        retry: Option<Box<Retry>>,
    ) -> Task<Message> {
        match result {
            Ok(helper) => {
                self.flash(
                    NoteKind::Info,
                    format!(
                        "logged in to {} as {} — kept by git's `{helper}` helper",
                        account.host, account.username
                    ),
                );
                self.settings.remember_account(account);
                self.save_settings();
                match retry {
                    Some(retry) => self.run_purpose(retry.purpose, retry.values, Vec::new()),
                    None => Task::none(),
                }
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
