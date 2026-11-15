//! Commit message editor: summary length guide (50/72), and two actions —
//! Commit (⌘⏎) and Amend — each enabled only when it can run, with the
//! reason shown when it cannot.

use iced::widget::{column, row, space, text_editor};
use iced::{Background, Border, Element, Length};

use crate::feature::{Cx, Feature, Update};
use crate::theme::Tokens;
use crate::widgets::{self, colored, dim};

/// Conventional soft limit for the summary line.
const SUMMARY_SOFT: usize = 50;
/// Hard-ish limit most tools truncate at.
const SUMMARY_HARD: usize = 72;

#[derive(Debug, Default)]
pub struct State {
    content: text_editor::Content,
    committing: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Edit(text_editor::Action),
    /// Put the last commit's message in the editor (to amend it).
    RecallLast,
    /// `amend: false` commits the staged changes; `true` replaces the last
    /// commit with the staged changes and the message (or keeps its message
    /// when the editor is empty).
    Submit {
        amend: bool,
    },
}

#[derive(Debug, Clone)]
pub enum Output {
    /// An empty `message` with `amend` keeps the last commit's message.
    Commit { message: String, amend: bool },
    /// The app loads HEAD's message and calls [`State::set_last_message`].
    LoadHeadMessage,
}

/// What the commit box knows about the repository.
#[derive(Debug, Clone, Copy)]
struct Repo {
    staged: usize,
    /// There is a commit to amend (not before the first commit).
    has_head: bool,
}

impl State {
    fn message(&self) -> String {
        self.content.text()
    }

    fn has_message(&self) -> bool {
        !self.message().trim().is_empty()
    }

    pub fn set_last_message(&mut self, message: Option<String>) {
        if let Some(m) = message {
            self.content = text_editor::Content::with_text(&m);
        }
    }

    /// Called after a successful commit.
    pub fn committed(&mut self) {
        self.content = text_editor::Content::new();
        self.committing = false;
    }

    pub fn commit_failed(&mut self) {
        self.committing = false;
    }

    /// Why a new commit is not possible right now, if it is not.
    fn commit_blocked(&self, repo: Repo) -> Option<&'static str> {
        if self.committing {
            Some("committing…")
        } else if repo.staged == 0 {
            Some("nothing staged — stage files first (Space, +, stage all)")
        } else if !self.has_message() {
            Some("write a commit message")
        } else {
            None
        }
    }

    /// Why amending is not possible right now, if it is not. Amend works
    /// with staged changes, a new message, or both.
    fn amend_blocked(&self, repo: Repo) -> Option<&'static str> {
        if self.committing {
            Some("committing…")
        } else if !repo.has_head {
            Some("no commit to amend yet")
        } else if repo.staged == 0 && !self.has_message() {
            Some("stage changes or write a new message to amend")
        } else {
            None
        }
    }

    fn repo(cx: &Cx<'_>) -> Option<Repo> {
        cx.snapshot.map(|s| Repo {
            staged: s.staged_count(),
            has_head: s.status.head.oid.is_some(),
        })
    }
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message {
            Message::Edit(action) => {
                self.content.perform(action);
                Update::none()
            }
            Message::RecallLast => Update::output(Output::LoadHeadMessage),
            Message::Submit { amend } => {
                // The view disables the buttons, but ⌘⏎ can still arrive;
                // git reports what is missing (e.g. nothing staged).
                if self.committing || (!amend && !self.has_message()) {
                    return Update::none();
                }
                self.committing = true;
                Update::output(Output::Commit {
                    message: self.message(),
                    amend,
                })
            }
        }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let repo = Self::repo(&cx);

        let editor = text_editor(&self.content)
            .placeholder("summary (≤50 chars)\n\nwhy this change is needed…")
            .on_action(Message::Edit)
            .size(t.font_size)
            .height(Length::Fixed(t.font_size * 7.0))
            .key_binding(|key| {
                use iced::keyboard::{Key, key::Named};
                if key.modifiers.command() && matches!(key.key.as_ref(), Key::Named(Named::Enter)) {
                    Some(text_editor::Binding::Custom(Message::Submit {
                        amend: false,
                    }))
                } else {
                    text_editor::Binding::from_key_press(key)
                }
            })
            .style(editor_style(t));

        let message = self.message();
        let summary_len = message.lines().next().map_or(0, |l| l.chars().count());
        let len_color = if summary_len > SUMMARY_HARD {
            t.error()
        } else if summary_len > SUMMARY_SOFT {
            t.warning()
        } else {
            t.dim
        };
        let loading = Some("loading…");
        let commit_blocked = repo.map_or(loading, |r| self.commit_blocked(r));
        let amend_blocked = repo.map_or(loading, |r| self.amend_blocked(r));
        let has_head = repo.is_some_and(|r| r.has_head);

        let footer = row![
            colored(t, format!("{summary_len}/{SUMMARY_SOFT}"), len_color),
            dim(t, format!("· {} staged", repo.map_or(0, |r| r.staged))),
            space().width(Length::Fill),
            widgets::tip(
                t,
                widgets::text_button(
                    t,
                    "last message",
                    None,
                    (has_head && !self.committing).then_some(Message::RecallLast),
                ),
                "put the last commit's message here to edit it for an amend".into(),
            ),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        // The line explains the main action (commit); amend's own reason is
        // on its tooltip.
        let buttons = row![
            dim(
                t,
                commit_blocked.map(|r| format!("  {r}")).unwrap_or_default()
            )
            .width(Length::Fill),
            widgets::tip(
                t,
                widgets::text_button(
                    t,
                    "Amend",
                    None,
                    amend_blocked
                        .is_none()
                        .then_some(Message::Submit { amend: true }),
                ),
                amend_blocked.map_or_else(
                    || {
                        if self.has_message() {
                            "replace the last commit: staged changes + this message".to_owned()
                        } else {
                            "add the staged changes to the last commit, keeping its message"
                                .to_owned()
                        }
                    },
                    str::to_owned,
                ),
            ),
            widgets::tip(
                t,
                widgets::text_button(
                    t,
                    "Commit",
                    Some("⌘⏎"),
                    commit_blocked
                        .is_none()
                        .then_some(Message::Submit { amend: false }),
                ),
                commit_blocked.map_or_else(
                    || "create a new commit from the staged changes".to_owned(),
                    str::to_owned,
                ),
            ),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);

        widgets::panel(
            t,
            "COMMIT".into(),
            column![editor, footer, buttons].spacing(4),
        )
    }
}

fn editor_style(
    t: &Tokens,
) -> impl Fn(&iced::Theme, text_editor::Status) -> text_editor::Style + '_ {
    move |_, status| text_editor::Style {
        background: Background::Color(t.bg),
        border: Border {
            color: if matches!(status, text_editor::Status::Focused { .. }) {
                t.accent()
            } else {
                t.border
            },
            width: 1.0,
            radius: 0.0.into(),
        },
        placeholder: t.dim,
        value: t.fg,
        selection: t.selection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> State {
        State {
            content: text_editor::Content::with_text(text),
            ..State::default()
        }
    }

    const REPO: Repo = Repo {
        staged: 1,
        has_head: true,
    };

    #[test]
    fn commit_needs_staged_changes_and_a_message() {
        let nothing = Repo { staged: 0, ..REPO };
        assert!(typed("msg").commit_blocked(nothing).is_some());
        assert!(typed("  ").commit_blocked(REPO).is_some());
        assert_eq!(typed("msg").commit_blocked(REPO), None);
    }

    #[test]
    fn amend_needs_a_commit_and_something_to_change() {
        let nothing = Repo { staged: 0, ..REPO };
        // Message only, staged changes only, or both.
        assert_eq!(typed("new msg").amend_blocked(nothing), None);
        assert_eq!(typed("").amend_blocked(REPO), None);
        assert!(typed("").amend_blocked(nothing).is_some());
        let unborn = Repo {
            has_head: false,
            ..REPO
        };
        assert!(typed("msg").amend_blocked(unborn).is_some());
    }

    #[test]
    fn submit_emits_once_until_done() {
        let mut s = typed("feat: x");
        assert!(matches!(
            s.update(Message::Submit { amend: false }).output,
            Some(Output::Commit { amend: false, .. })
        ));
        assert!(
            s.update(Message::Submit { amend: true }).output.is_none(),
            "no double commit"
        );
        s.committed();
        assert!(s.message().trim().is_empty());
        // An empty editor amends keeping the last message; it never commits.
        assert!(s.update(Message::Submit { amend: false }).output.is_none());
        assert!(matches!(
            s.update(Message::Submit { amend: true }).output,
            Some(Output::Commit { amend: true, ref message }) if message.trim().is_empty()
        ));
    }

    #[test]
    fn last_message_fills_the_editor() {
        let mut s = typed("draft");
        assert!(matches!(
            s.update(Message::RecallLast).output,
            Some(Output::LoadHeadMessage)
        ));
        s.set_last_message(Some("previous message".into()));
        assert!(s.message().starts_with("previous message"));
    }
}
