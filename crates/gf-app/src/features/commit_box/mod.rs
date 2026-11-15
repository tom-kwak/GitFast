//! Commit message: a one-line summary (with the 50/72 length guide) and a
//! multi-line body for the details, plus Commit (⌘⏎) and Amend — each
//! enabled only when it can run, the reason on its tooltip.

use iced::widget::{column, row, space, text_editor, text_input};
use iced::{Background, Border, Element, Length};

use crate::feature::{Cx, Feature, Update};
use crate::theme::{Tokens, style};
use crate::widgets::{self, colored};

/// Conventional soft limit for the summary line.
const SUMMARY_SOFT: usize = 50;
/// Hard-ish limit most tools truncate at.
const SUMMARY_HARD: usize = 72;

/// Widget ids, so Enter in the summary moves to the body.
pub const SUMMARY_ID: &str = "gf-commit-summary";
const BODY_ID: &str = "gf-commit-body";

#[derive(Debug, Default)]
pub struct State {
    summary: String,
    body: text_editor::Content,
    committing: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Summary(String),
    /// Enter in the summary: continue in the body.
    SummaryDone,
    Body(text_editor::Action),
    /// Put the last commit's message in the fields (to amend it).
    RecallLast,
    /// `amend: false` commits the staged changes; `true` replaces the last
    /// commit with the staged changes and the message (or keeps its message
    /// when the summary is empty).
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
    fn body_text(&self) -> String {
        self.body.text().trim_end().to_owned()
    }

    /// The full message: summary, blank line, body. Empty without a
    /// summary (a body alone is not a message).
    fn message(&self) -> String {
        let summary = self.summary.trim();
        if summary.is_empty() {
            return String::new();
        }
        let body = self.body_text();
        if body.trim().is_empty() {
            summary.to_owned()
        } else {
            format!("{summary}\n\n{body}")
        }
    }

    fn has_message(&self) -> bool {
        !self.summary.trim().is_empty()
    }

    /// Details typed without a summary line.
    fn body_without_summary(&self) -> bool {
        !self.has_message() && !self.body_text().trim().is_empty()
    }

    /// Fills the fields from a full message (first paragraph = summary).
    pub fn set_last_message(&mut self, message: Option<String>) {
        let Some(m) = message else { return };
        let (summary, body) = match m.split_once("\n\n") {
            Some((s, b)) => (s, b.trim_start_matches('\n')),
            None => (m.as_str(), ""),
        };
        self.summary = summary.lines().map(str::trim).collect::<Vec<_>>().join(" ");
        self.body = text_editor::Content::with_text(body);
    }

    /// Called after a successful commit.
    pub fn committed(&mut self) {
        self.summary.clear();
        self.body = text_editor::Content::new();
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
            Some("write a summary line")
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
        } else if self.body_without_summary() {
            Some("write a summary line for the new message")
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
            Message::Summary(s) => {
                // One line: pasted newlines become spaces.
                self.summary = s.replace(['\r', '\n'], " ");
                Update::none()
            }
            Message::SummaryDone => Update::task(iced::widget::operation::focus(BODY_ID)),
            Message::Body(action) => {
                self.body.perform(action);
                Update::none()
            }
            Message::RecallLast => Update::output(Output::LoadHeadMessage),
            Message::Submit { amend } => {
                // The view disables the buttons, but ⌘⏎ can still arrive;
                // git reports what is missing (e.g. nothing staged).
                let blocked = if amend {
                    self.body_without_summary()
                } else {
                    !self.has_message()
                };
                if self.committing || blocked {
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

        let len = self.summary.chars().count();
        let len_color = if len > SUMMARY_HARD {
            t.error()
        } else if len > SUMMARY_SOFT {
            t.warning()
        } else {
            t.dim
        };
        let summary = row![
            text_input("summary — what changed", &self.summary)
                .id(SUMMARY_ID)
                .on_input(Message::Summary)
                .on_submit(Message::SummaryDone)
                .size(t.font_size)
                .padding(4)
                .style(style::field_input(t)),
            colored(t, format!("{len}/{SUMMARY_SOFT}"), len_color),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        let body = text_editor(&self.body)
            .id(BODY_ID)
            .placeholder("details (optional) — why, and what a reviewer should know")
            .on_action(Message::Body)
            .size(t.font_size)
            .height(Length::Fill)
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

        let loading = Some("loading…");
        let commit_blocked = repo.map_or(loading, |r| self.commit_blocked(r));
        let amend_blocked = repo.map_or(loading, |r| self.amend_blocked(r));
        let has_head = repo.is_some_and(|r| r.has_head);
        let staged = repo.map_or(0, |r| r.staged);

        let (status, status_color) = if staged == 0 {
            ("0 staged".to_owned(), t.warning())
        } else {
            (format!("{staged} staged"), t.added())
        };
        let action = |label: &str, key: Option<&str>, on: Option<Message>, tip: String| {
            widgets::tip(t, widgets::text_button(t, label, key, on), tip)
        };
        let buttons = row![
            colored(t, status, status_color),
            space().width(Length::Fill),
            widgets::icon_button(
                t,
                "↺",
                if has_head { t.accent() } else { t.dim },
                "fill in the last commit's message (to amend it)",
                Message::RecallLast,
            ),
            action(
                "Amend",
                None,
                amend_blocked
                    .is_none()
                    .then_some(Message::Submit { amend: true }),
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
            action(
                "Commit",
                None,
                commit_blocked
                    .is_none()
                    .then_some(Message::Submit { amend: false }),
                commit_blocked.map_or_else(
                    || "create a new commit from the staged changes · ⌘⏎".to_owned(),
                    str::to_owned,
                ),
            ),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);

        widgets::panel(
            t,
            "COMMIT".into(),
            column![summary, body, buttons].spacing(4),
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

    fn typed(summary: &str) -> State {
        State {
            summary: summary.to_owned(),
            ..State::default()
        }
    }

    const REPO: Repo = Repo {
        staged: 1,
        has_head: true,
    };

    #[test]
    fn commit_needs_staged_changes_and_a_summary() {
        let nothing = Repo { staged: 0, ..REPO };
        assert!(typed("msg").commit_blocked(nothing).is_some());
        assert!(typed("  ").commit_blocked(REPO).is_some());
        assert_eq!(typed("msg").commit_blocked(REPO), None);
    }

    #[test]
    fn amend_needs_a_commit_and_something_to_change() {
        let nothing = Repo { staged: 0, ..REPO };
        assert_eq!(typed("new msg").amend_blocked(nothing), None);
        assert_eq!(typed("").amend_blocked(REPO), None);
        assert!(typed("").amend_blocked(nothing).is_some());
        let unborn = Repo {
            has_head: false,
            ..REPO
        };
        assert!(typed("msg").amend_blocked(unborn).is_some());
        // Details without a summary would be dropped: ask for a summary.
        let mut body_only = typed("");
        body_only.body = text_editor::Content::with_text("why");
        assert!(body_only.amend_blocked(REPO).is_some());
    }

    #[test]
    fn message_is_summary_blank_line_body() {
        let mut s = typed("  feat: x  ");
        assert_eq!(s.message(), "feat: x");
        s.body = text_editor::Content::with_text("because y\n\nmore\n");
        assert_eq!(s.message(), "feat: x\n\nbecause y\n\nmore");
        // Pasting several lines into the summary keeps it one line.
        let _ = s.update(Message::Summary("a\nb".into()));
        assert_eq!(s.summary, "a b");
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
        assert!(s.message().is_empty());
        // An empty summary amends keeping the last message; it never commits.
        assert!(s.update(Message::Submit { amend: false }).output.is_none());
        assert!(matches!(
            s.update(Message::Submit { amend: true }).output,
            Some(Output::Commit { amend: true, ref message }) if message.is_empty()
        ));
    }

    #[test]
    fn last_message_fills_summary_and_body() {
        let mut s = typed("draft");
        assert!(matches!(
            s.update(Message::RecallLast).output,
            Some(Output::LoadHeadMessage)
        ));
        s.set_last_message(Some("previous\nsummary\n\nthe details".into()));
        assert_eq!(s.summary, "previous summary");
        assert_eq!(s.body_text(), "the details");
        assert_eq!(s.message(), "previous summary\n\nthe details");
    }
}
