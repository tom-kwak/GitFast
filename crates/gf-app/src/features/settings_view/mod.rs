//! App-wide preferences: a section list on the left (↑/↓, ⌘1–⌘4) and the
//! chosen section on the right.
//!
//! ACCOUNTS lists the git servers logged in to with whether git's
//! credential helper still holds their token, the servers the open
//! repositories use without a login (one click to log in), and SSH
//! servers, which use the user's SSH keys. The other sections make the
//! `:` prompt settings (`:theme`, `:editor`, `:terminal`,
//! `:confirm reset`) clickable.

pub mod provider;

use std::collections::BTreeMap;

use gf_config::{Account, Settings};
use iced::widget::{Column, Row, button, column, container, row, space};
use iced::{Alignment, Element, Length, Padding};

use crate::theme::{self, Tokens, style};
use crate::widgets::{self, colored, dim, mono};
pub use provider::Provider;

/// Smallest and largest font size offered.
pub const FONT_SIZES: std::ops::RangeInclusive<u16> = 10..=20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Accounts,
    Appearance,
    Tools,
    Confirmations,
}

impl Section {
    pub const ALL: [Self; 4] = [
        Self::Accounts,
        Self::Appearance,
        Self::Tools,
        Self::Confirmations,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Accounts => "ACCOUNTS",
            Self::Appearance => "APPEARANCE",
            Self::Tools => "TOOLS",
            Self::Confirmations => "CONFIRMATIONS",
        }
    }

    /// The `n`th section (0-based), for ⌘1–⌘4.
    pub fn nth(n: usize) -> Option<Self> {
        Self::ALL.get(n).copied()
    }

    /// The one above (`back`) or below; stays at either end.
    pub fn step(self, back: bool) -> Self {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        let next = if back {
            i.saturating_sub(1)
        } else {
            (i + 1).min(Self::ALL.len() - 1)
        };
        Self::ALL[next]
    }
}

/// Something read in the background for the page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Probe<T> {
    #[default]
    Checking,
    Known(T),
    Failed(String),
}

/// A server the open repositories' remotes point at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerUse {
    /// `https`, `http` or `ssh`.
    pub protocol: String,
    pub host: String,
    /// The first remote URL on it, without a user name (checks a login).
    pub url: String,
    /// `repo · remote` for each remote on it.
    pub used_by: Vec<String>,
}

impl ServerUse {
    /// As typed in the log-in dialog: `https` is left out.
    pub fn label(&self) -> String {
        if self.protocol == "https" {
            self.host.clone()
        } else {
            format!("{}://{}", self.protocol, self.host)
        }
    }
}

/// The page's own state; the values themselves are in [`Settings`].
#[derive(Debug, Default)]
pub struct Page {
    pub section: Section,
    /// HTTP(S) servers of the open tabs' remotes.
    pub servers: Vec<ServerUse>,
    /// SSH servers of the open tabs' remotes.
    pub ssh: Vec<ServerUse>,
    /// git's credential helper (`None`: there is none).
    pub helper: Probe<Option<String>>,
    /// Whether the helper still has each account's token.
    pub tokens: Vec<(Account, Probe<bool>)>,
}

impl Page {
    pub fn token(&self, account: &Account) -> &Probe<bool> {
        static CHECKING: Probe<bool> = Probe::Checking;
        self.tokens
            .iter()
            .find(|(a, _)| a == account)
            .map_or(&CHECKING, |(_, probe)| probe)
    }

    fn used_by(&self, protocol: &str, host: &str) -> &[String] {
        self.servers
            .iter()
            .find(|s| s.protocol == protocol && s.host == host)
            .map_or(&[], |s| s.used_by.as_slice())
    }
}

/// What the page shows besides its own state.
pub struct Props<'a> {
    pub page: &'a Page,
    pub settings: &'a Settings,
    /// The git program gf runs (named when it is missing).
    pub git: String,
    pub git_missing: bool,
    /// The credential helper this platform offers when git has none:
    /// (`credential.helper` value, its name).
    pub offer_helper: Option<(&'static str, &'static str)>,
    /// The system terminal's name.
    pub system_terminal: &'static str,
}

#[derive(Debug, Clone)]
pub enum Message {
    Section(Section),
    SetTheme(&'static str),
    /// Font size one step smaller (`-1`) or larger (`1`).
    FontSize(i8),
    /// Open the log-in dialog empty.
    AddAccount,
    /// Open the log-in dialog for this server (as typed there).
    LogInTo(String),
    /// Log in to `accounts[i]` again: a new token for the same user.
    UpdateToken(usize),
    /// Log out of `accounts[i]` (asks first).
    LogOut(usize),
    /// Ask which hosting service this host is (its tag was clicked).
    PickService(String),
    /// Make git keep logins with this credential helper (asks first).
    UseHelper(&'static str),
    EditEditor,
    EditTerminal,
    EditWorktreeRoot,
    /// Turn copying `.worktreeinclude` files into new worktrees on or off.
    ToggleCopyIncluded,
    /// Ask again before `settings.confirm.skip[i]`.
    AskAgain(usize),
    AskAgainAll,
    Close,
}

pub fn view<'a>(t: &'a Tokens, p: Props<'a>) -> Element<'a, Message> {
    let header = row![
        colored(t, "Settings", t.accent()),
        dim(t, concat!("gf v", env!("CARGO_PKG_VERSION"))),
        space().width(Length::Fill),
        widgets::text_button(t, "Done", Some("Esc"), Some(Message::Close)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let section = p.page.section;
    let nav = Column::with_children(Section::ALL.iter().enumerate().map(|(i, &s)| {
        let selected = s == section;
        let label = row![
            colored(
                t,
                if selected { "▸" } else { " " },
                if selected { t.accent() } else { t.dim }
            ),
            mono(t, s.title()),
            space().width(Length::Fill),
            dim(t, format!("⌘{}", i + 1)),
        ]
        .spacing(6);
        widgets::list_row(t, label, selected, Message::Section(s))
    }))
    .spacing(2)
    .push(space().height(Length::Fill))
    .push(dim(t, "↑/↓ section"));

    let content: Element<'a, Message> = match section {
        Section::Accounts => accounts(t, &p),
        Section::Appearance => appearance(t, p.settings),
        Section::Tools => tools(t, &p),
        Section::Confirmations => confirmations(t, p.settings),
    };
    let body = row![
        container(nav).width(t.cols(20)).height(Length::Fill),
        vline(t),
        widgets::scroll(container(content).padding(Padding {
            top: 0.0,
            right: 12.0,
            bottom: 4.0,
            left: 0.0,
        })),
    ]
    .spacing(12)
    .height(Length::Fixed((t.font_size * 30.0).round()));

    // gf can't do anything without git: say so above every section.
    let missing = p.git_missing.then(|| {
        colored(
            t,
            format!(
                "⚠ git was not found (`{}`). Install git, then restart gf.",
                p.git
            ),
            t.warning(),
        )
    });
    container(
        Column::new()
            .push(header)
            .extend(missing.map(Element::from))
            .push(hline(t))
            .push(body)
            .spacing(10),
    )
    .padding(Padding::from([12, 16]))
    .width(Length::Fill)
    .max_width(t.cols(104))
    .style(style::accent_panel(t, t.accent()))
    .into()
}

fn hline<'a>(t: &'a Tokens) -> Element<'a, Message> {
    let color = t.border;
    container(space().width(Length::Fill).height(1))
        .style(move |_| container::Style::default().background(color))
        .into()
}

fn vline<'a>(t: &'a Tokens) -> Element<'a, Message> {
    let color = t.border;
    container(space().width(1).height(Length::Fill))
        .style(move |_| container::Style::default().background(color))
        .into()
}

/// `─ TITLE  note` with an optional action on the right.
fn heading<'a>(
    t: &'a Tokens,
    title: &'a str,
    note: Option<&'a str>,
    action: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut line = row![dim(t, format!("─ {title}"))]
        .spacing(10)
        .align_y(Alignment::Center);
    if let Some(note) = note {
        line = line.push(dim(t, note));
    }
    line = line.push(space().width(Length::Fill));
    if let Some(action) = action {
        line = line.push(action);
    }
    line.into()
}

/// A label column so values line up.
fn labeled<'a>(
    t: &'a Tokens,
    label: &'a str,
    value: impl Into<Element<'a, Message>>,
) -> Row<'a, Message> {
    row![container(dim(t, label)).width(t.cols(15)), value.into()]
        .spacing(8)
        .align_y(Alignment::Center)
}

fn used_by_line<'a>(t: &'a Tokens, used_by: &[String]) -> Element<'a, Message> {
    if used_by.is_empty() {
        dim(t, "no open repository uses it").into()
    } else {
        dim(t, format!("used by {}", used_by.join(", "))).into()
    }
}

fn accounts<'a>(t: &'a Tokens, p: &Props<'a>) -> Element<'a, Message> {
    let page = p.page;
    let accounts = &p.settings.accounts;
    let mut col = Column::new().spacing(10).push(heading(
        t,
        "ACCOUNTS",
        Some("HTTPS logins"),
        Some(widgets::text_button(
            t,
            "+ Add account",
            None,
            Some(Message::AddAccount),
        )),
    ));

    if accounts.is_empty() {
        col = col.push(dim(
            t,
            "No accounts yet. Add one here, or push/fetch: gf asks when a server wants a login.",
        ));
    }
    let services = &p.settings.services;
    for (i, account) in accounts.iter().enumerate() {
        col = col.push(account_card(t, page, services, i, account));
    }

    let others: Vec<&ServerUse> = page
        .servers
        .iter()
        .filter(|s| {
            !accounts
                .iter()
                .any(|a| a.protocol == s.protocol && a.host == s.host)
        })
        .collect();
    if !others.is_empty() {
        col = col.push(heading(
            t,
            "NOT LOGGED IN",
            Some("used by the open repositories"),
            None,
        ));
        for server in others {
            col = col.push(server_card(t, services, server));
        }
    }
    if !page.ssh.is_empty() {
        col = col.push(heading(t, "SSH", Some("log in with your SSH keys"), None));
        for server in &page.ssh {
            col = col.push(
                column![
                    mono(t, server.host.as_str()),
                    container(used_by_line(t, &server.used_by)).padding(Padding {
                        left: t.cols(2),
                        ..Padding::ZERO
                    }),
                ]
                .spacing(2),
            );
        }
    }
    col.push(helper_line(t, p)).into()
}

/// The host's service; a click asks which one it is (the name can
/// mislead, or tell nothing).
fn provider_tag<'a>(
    t: &'a Tokens,
    services: &BTreeMap<String, String>,
    host: &str,
) -> Element<'a, Message> {
    let provider = Provider::for_host(services, host);
    let tag = button(colored(t, provider.name(), t.accent()))
        .padding(0)
        .style(style::text_button(t, t.accent()))
        .on_press(Message::PickService(host.to_owned()));
    container(widgets::tip(t, tag, "which service is it?".into()))
        .width(t.cols(10))
        .into()
}

fn account_card<'a>(
    t: &'a Tokens,
    page: &Page,
    services: &BTreeMap<String, String>,
    i: usize,
    account: &'a Account,
) -> Element<'a, Message> {
    let token = page.token(account);
    let missing = matches!(token, Probe::Known(false));
    // The token page is offered in the log-in dialog, where the token
    // is typed: a saved login has no use for it.
    let top = row![
        provider_tag(t, services, &account.host),
        mono(t, server_label(account)),
        dim(t, "as"),
        mono(t, account.username.as_str()),
        space().width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .push(widgets::text_button(
        t,
        if missing { "Log in again" } else { "New token" },
        None,
        Some(Message::UpdateToken(i)),
    ))
    .push(widgets::danger_button(
        t,
        "Log out",
        Some(Message::LogOut(i)),
    ));

    let status: Element<'a, Message> = match token {
        Probe::Checking => dim(t, "… checking the saved token").into(),
        Probe::Known(true) => colored(t, "✓ token saved", t.success()).into(),
        Probe::Known(false) => colored(
            t,
            "⚠ no saved token (expired, revoked or removed)",
            t.warning(),
        )
        .into(),
        Probe::Failed(e) => dim(t, format!("? could not check the token: {e}")).into(),
    };
    let detail = column![
        status,
        used_by_line(t, page.used_by(&account.protocol, &account.host))
    ]
    .spacing(2);
    column![
        top,
        container(detail).padding(Padding {
            left: t.cols(11),
            ..Padding::ZERO
        })
    ]
    .spacing(2)
    .into()
}

fn server_card<'a>(
    t: &'a Tokens,
    services: &BTreeMap<String, String>,
    server: &ServerUse,
) -> Element<'a, Message> {
    let top = row![
        provider_tag(t, services, &server.host),
        mono(t, server.label()),
        space().width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .push(widgets::text_button(
        t,
        "Log in",
        None,
        Some(Message::LogInTo(server.label())),
    ));
    column![
        top,
        container(used_by_line(t, &server.used_by)).padding(Padding {
            left: t.cols(11),
            ..Padding::ZERO
        })
    ]
    .spacing(2)
    .into()
}

/// Where tokens are kept, or a one-click fix when git keeps none.
fn helper_line<'a>(t: &'a Tokens, p: &Props<'a>) -> Element<'a, Message> {
    match &p.page.helper {
        Probe::Checking => dim(t, "… reading git's credential helper").into(),
        Probe::Failed(e) => dim(t, format!("? could not read git's credential helper: {e}")).into(),
        Probe::Known(Some(helper)) => {
            // `store --file=/long/path` reads as `store`.
            let name = helper.split_whitespace().next().unwrap_or(helper);
            widgets::tip(
                t,
                dim(
                    t,
                    format!(
                        "Tokens are kept by git's credential helper `{name}`, not by gf; \
                         git in the terminal uses them too."
                    ),
                ),
                format!("credential.helper = {helper}"),
            )
        }
        Probe::Known(None) => {
            let warning = colored(
                t,
                "⚠ git has no credential helper, so it can't keep a login.",
                t.warning(),
            );
            let fix: Element<'a, Message> = match p.offer_helper {
                Some((helper, name)) => widgets::text_button(
                    t,
                    &format!("Use {name}"),
                    None,
                    Some(Message::UseHelper(helper)),
                ),
                None => dim(t, "git config --global credential.helper <helper>").into(),
            };
            // The fix on its own line: beside the warning it wrapped.
            column![warning, fix].spacing(4).into()
        }
    }
}

fn server_label(account: &Account) -> String {
    if account.protocol == "https" {
        account.host.clone()
    } else {
        format!("{}://{}", account.protocol, account.host)
    }
}

fn appearance<'a>(t: &'a Tokens, settings: &'a Settings) -> Element<'a, Message> {
    let themes = Column::with_children(theme::PRESET_NAMES.iter().map(|&name| theme_row(t, name)))
        .spacing(2);
    let size = settings.appearance.font_size;
    let smaller = (size > *FONT_SIZES.start()).then_some(Message::FontSize(-1));
    let larger = (size < *FONT_SIZES.end()).then_some(Message::FontSize(1));
    let font = row![
        widgets::text_button(t, "−", None, smaller),
        mono(t, format!("{size} pt")),
        widgets::text_button(t, "+", None, larger),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    column![
        heading(t, "THEME", Some("←/→ to change"), None),
        themes,
        heading(t, "FONT SIZE", None, None),
        font,
    ]
    .spacing(10)
    .into()
}

fn theme_row<'a>(t: &'a Tokens, name: &'static str) -> Element<'a, Message> {
    let selected = t.name == name;
    let preview = theme::preset(name);
    let swatches = [preview.bg, preview.fg, preview.accent(), preview.added()]
        .into_iter()
        .fold(Row::new().spacing(2), |swatches, color| {
            swatches.push(
                container(space().width(12).height(12))
                    .style(move |_| container::Style::default().background(color)),
            )
        });
    let content = row![
        colored(
            t,
            if selected { "●" } else { "○" },
            if selected { t.accent() } else { t.dim }
        ),
        mono(t, name),
        space().width(Length::Fill),
        swatches,
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    button(content)
        .width(Length::Fill)
        .padding(Padding::from([5, 8]))
        .style(style::row_button(t, selected))
        .on_press_maybe((!selected).then_some(Message::SetTheme(name)))
        .into()
}

fn tools<'a>(t: &'a Tokens, p: &Props<'a>) -> Element<'a, Message> {
    let wt = &p.settings.worktrees;
    let change = |m: Message| widgets::text_button(t, "Change", None, Some(m));
    let terminal = match &wt.terminal {
        Some(app) => mono(t, app.clone()),
        None => mono(t, format!("{} (the system's)", p.system_terminal)),
    };
    let root = match &wt.root {
        Some(root) => mono(t, widgets::format::tilde(root)),
        None => mono(t, "<repository>-worktrees/ beside each repository"),
    };
    let item = |label: &'a str, value: Element<'a, Message>, note: String, m: Message| {
        column![
            labeled(
                t,
                label,
                row![value, space().width(Length::Fill), change(m)]
            ),
            row![space().width(t.cols(15)), dim(t, note)].spacing(8),
        ]
        .spacing(2)
    };
    column![
        heading(t, "OPEN WORKTREES WITH", None, None),
        item(
            "editor",
            mono(t, wt.editor.as_str()).into(),
            format!("runs `{} <folder>`", wt.editor),
            Message::EditEditor,
        ),
        item(
            "terminal",
            terminal.into(),
            "the app a worktree opens in".into(),
            Message::EditTerminal,
        ),
        heading(t, "NEW WORKTREES", None, None),
        item(
            "folder",
            root.into(),
            "where `worktree add` puts new trees".into(),
            Message::EditWorktreeRoot,
        ),
        column![
            labeled(
                t,
                "local files",
                row![
                    if wt.copy_included {
                        mono(t, "copy what .worktreeinclude lists")
                    } else {
                        dim(t, "not copied")
                    },
                    space().width(Length::Fill),
                    widgets::text_button(
                        t,
                        if wt.copy_included {
                            "Turn off"
                        } else {
                            "Turn on"
                        },
                        None,
                        Some(Message::ToggleCopyIncluded),
                    ),
                ]
            ),
            row![
                space().width(t.cols(15)),
                dim(
                    t,
                    "ignored files (.env …) the repository's .worktreeinclude lists"
                ),
            ]
            .spacing(8),
        ]
        .spacing(2),
    ]
    .spacing(10)
    .into()
}

fn confirmations<'a>(t: &'a Tokens, settings: &'a Settings) -> Element<'a, Message> {
    let skipped = &settings.confirm.skip;
    let action = (!skipped.is_empty())
        .then(|| widgets::text_button(t, "Ask again for all", None, Some(Message::AskAgainAll)));
    let mut col = Column::new()
        .spacing(10)
        .push(heading(t, "CONFIRMATIONS", None, action))
        .push(dim(
            t,
            "Changes ask before they run until you tick \"don't ask again\". Deletions always ask.",
        ));
    if skipped.is_empty() {
        col = col.push(colored(t, "✓ every change asks first", t.success()));
    }
    for (i, key) in skipped.iter().enumerate() {
        col = col.push(
            row![
                dim(t, "not asking before"),
                mono(t, key.replace('-', " ")),
                space().width(Length::Fill),
                widgets::text_button(t, "Ask again", None, Some(Message::AskAgain(i))),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    col.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_step_and_stop_at_the_ends() {
        assert_eq!(Section::Accounts.step(true), Section::Accounts);
        assert_eq!(Section::Accounts.step(false), Section::Appearance);
        assert_eq!(Section::Confirmations.step(false), Section::Confirmations);
        assert_eq!(Section::nth(3), Some(Section::Confirmations));
        assert_eq!(Section::nth(4), None);
    }
}
