//! Commit message editor: summary length guide (50/72), amend toggle and
//! ⌘⏎ to commit. The button explains why it is disabled.

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
    amend: bool,
    /// Draft kept while amend replaced the editor text, restored when amend
    /// is switched off.
    draft_before_amend: Option<String>,
    committing: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Edit(text_editor::Action),
    ToggleAmend,
    Submit,
}

#[derive(Debug, Clone)]
pub enum Output {
    Commit {
        message: String,
        amend: bool,
    },
    /// Amend was switched on: the app loads HEAD's message and calls
    /// [`State::set_amend_message`].
    LoadHeadMessage,
}

impl State {
    fn message(&self) -> String {
        self.content.text()
    }

    pub fn set_amend_message(&mut self, message: Option<String>) {
        if !self.amend {
            return;
        }
        if let Some(m) = message {
            self.draft_before_amend = Some(self.message());
            self.content = text_editor::Content::with_text(&m);
        }
    }

    /// Called after a successful commit.
    pub fn committed(&mut self) {
        self.content = text_editor::Content::new();
        self.amend = false;
        self.draft_before_amend = None;
        self.committing = false;
    }

    pub fn commit_failed(&mut self) {
        self.committing = false;
    }

    /// Why committing is not possible right now, if it is not.
    fn blocked_reason(&self, staged: usize) -> Option<&'static str> {
        if self.committing {
            Some("committing…")
        } else if staged == 0 && !self.amend {
            Some("stage changes to commit")
        } else if self.message().trim().is_empty() {
            Some("write a commit message")
        } else {
            None
        }
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
            Message::ToggleAmend => {
                self.amend = !self.amend;
                if self.amend {
                    Update::output(Output::LoadHeadMessage)
                } else {
                    if let Some(draft) = self.draft_before_amend.take() {
                        self.content = text_editor::Content::with_text(&draft);
                    }
                    Update::none()
                }
            }
            Message::Submit => {
                // The view disables the button, but ⌘⏎ can still arrive.
                let message = self.message();
                if self.committing || message.trim().is_empty() {
                    return Update::none();
                }
                self.committing = true;
                Update::output(Output::Commit {
                    message,
                    amend: self.amend,
                })
            }
        }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let staged = cx.snapshot.map_or(0, |s| s.staged_count());

        let editor = text_editor(&self.content)
            .placeholder("summary (≤50 chars)\n\nwhy this change is needed…")
            .on_action(Message::Edit)
            .size(t.font_size)
            .height(Length::Fixed(t.font_size * 7.0))
            .key_binding(|key| {
                use iced::keyboard::{Key, key::Named};
                if key.modifiers.command() && matches!(key.key.as_ref(), Key::Named(Named::Enter)) {
                    Some(text_editor::Binding::Custom(Message::Submit))
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
        let blocked = cx
            .snapshot
            .map_or(Some("loading…"), |_| self.blocked_reason(staged));

        let amend = widgets::text_button(
            t,
            if self.amend {
                "■ amend last commit"
            } else {
                "□ amend last commit"
            },
            None,
            Some(Message::ToggleAmend),
        );
        let commit_label = if self.amend { "Amend" } else { "Commit" };
        let footer = row![
            colored(t, format!("{summary_len}/{SUMMARY_SOFT}"), len_color),
            dim(t, format!("· {staged} staged")),
            space().width(Length::Fill),
            amend,
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        let commit_row = row![
            dim(t, blocked.map(|r| format!("  {r}")).unwrap_or_default()).width(Length::Fill),
            widgets::text_button(
                t,
                commit_label,
                Some("⌘⏎"),
                blocked.is_none().then_some(Message::Submit)
            ),
        ]
        .align_y(iced::Alignment::Center);

        widgets::panel(
            t,
            "COMMIT".into(),
            column![editor, footer, commit_row].spacing(4),
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

    #[test]
    fn blocked_reasons() {
        assert_eq!(
            typed("msg").blocked_reason(0),
            Some("stage changes to commit")
        );
        assert_eq!(
            typed("  ").blocked_reason(2),
            Some("write a commit message")
        );
        assert_eq!(typed("msg").blocked_reason(1), None);
        let mut amend = typed("msg");
        amend.amend = true;
        assert_eq!(
            amend.blocked_reason(0),
            None,
            "amend may only change the message"
        );
    }

    #[test]
    fn submit_emits_once_until_done() {
        let mut s = typed("feat: x");
        assert!(matches!(
            s.update(Message::Submit).output,
            Some(Output::Commit { .. })
        ));
        assert!(
            s.update(Message::Submit).output.is_none(),
            "no double commit"
        );
        s.committed();
        assert!(s.message().trim().is_empty());
    }

    #[test]
    fn amend_toggle_restores_draft() {
        let mut s = typed("my draft");
        assert!(matches!(
            s.update(Message::ToggleAmend).output,
            Some(Output::LoadHeadMessage)
        ));
        s.set_amend_message(Some("previous message".into()));
        assert!(s.message().starts_with("previous message"));
        let _ = s.update(Message::ToggleAmend);
        assert!(s.message().starts_with("my draft"));
    }
}
