//! Modal dialog in TUI style: a bordered box with a title, explanation,
//! optional text fields and checkboxes, and `[ Cancel Esc ]` / `[ OK ⏎ ]`.
//!
//! [`Severity`] decides how careful the dialog is:
//!
//! | severity | look            | Enter confirms | "don't ask again" |
//! |----------|-----------------|----------------|-------------------|
//! | Normal   | accent (input)  | yes            | no                |
//! | Change   | yellow, `?`     | yes            | optional          |
//! | Delete   | red, `⚠`        | **no** — click | never             |
//!
//! Generic over the purpose `P`, which the app uses to know what to do on
//! submit. Apps should not build confirmations by hand: use the app's
//! shared `confirm` helper so every delete/change asks the same way.

use iced::widget::{Column, column, container, row, space, text_input};
use iced::{Background, Border, Color, Element, Length, Padding};

use crate::theme::{Tokens, style};
use crate::widgets::{self, colored, dim, mono};

/// Id of the first text field, focused when the dialog opens.
pub const FIRST_FIELD_ID: &str = "gf-dialog-field-0";

const FIELD_IDS: [&str; 4] = [
    FIRST_FIELD_ID,
    "gf-dialog-field-1",
    "gf-dialog-field-2",
    "gf-dialog-field-3",
];

/// How careful a dialog is (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Input dialogs (new branch, new tag…).
    Normal,
    /// Changes the working tree, HEAD or a remote (checkout, pull, push…).
    Change,
    /// Deletes something that cannot be brought back from the UI.
    Delete,
}

impl Severity {
    fn accent(self, t: &Tokens) -> Color {
        match self {
            Self::Normal => t.accent(),
            Self::Change => t.warning(),
            Self::Delete => t.error(),
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Normal => "",
            Self::Change => "? ",
            Self::Delete => "⚠ ",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Field {
    pub label: String,
    pub value: String,
    pub placeholder: String,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub label: String,
    pub value: bool,
}

#[derive(Debug, Clone)]
pub struct Dialog<P> {
    pub title: String,
    pub body: Vec<String>,
    pub fields: Vec<Field>,
    pub checks: Vec<Check>,
    pub confirm_label: String,
    pub severity: Severity,
    pub purpose: P,
    /// Validation message shown above the buttons.
    pub error: Option<String>,
    /// Key of a skippable confirmation; shows a "don't ask again" box.
    pub remember: Option<String>,
    dont_ask: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Field(usize, String),
    Check(usize),
    DontAsk,
    Submit,
    Cancel,
}

/// What the user decided.
#[derive(Debug, Clone)]
pub enum Outcome<P> {
    Submitted {
        purpose: P,
        values: Vec<String>,
        checks: Vec<bool>,
        /// Set when "don't ask again" was ticked: the confirmation key.
        dont_ask_again: Option<String>,
    },
    Cancelled {
        purpose: P,
    },
}

impl<P: Clone> Dialog<P> {
    pub fn confirm(
        title: impl Into<String>,
        body: Vec<String>,
        confirm_label: impl Into<String>,
        purpose: P,
    ) -> Self {
        Self {
            title: title.into(),
            body,
            fields: Vec::new(),
            checks: Vec::new(),
            confirm_label: confirm_label.into(),
            severity: Severity::Normal,
            purpose,
            error: None,
            remember: None,
            dont_ask: false,
        }
    }

    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Offer "don't ask again" (ignored for deletions, which always ask).
    pub fn remember_as(mut self, key: impl Into<String>) -> Self {
        if self.severity != Severity::Delete {
            self.remember = Some(key.into());
        }
        self
    }

    pub fn field(
        mut self,
        label: impl Into<String>,
        value: impl Into<String>,
        placeholder: impl Into<String>,
    ) -> Self {
        self.fields.push(Field {
            label: label.into(),
            value: value.into(),
            placeholder: placeholder.into(),
        });
        self
    }

    pub fn check(mut self, label: impl Into<String>, value: bool) -> Self {
        self.checks.push(Check {
            label: label.into(),
            value,
        });
        self
    }

    pub fn has_fields(&self) -> bool {
        !self.fields.is_empty()
    }

    /// Handles a message. `Some(outcome)` closes the dialog.
    pub fn update(&mut self, message: Message) -> Option<Outcome<P>> {
        match message {
            Message::Field(i, v) => {
                if let Some(f) = self.fields.get_mut(i) {
                    f.value = v;
                    self.error = None;
                }
                None
            }
            Message::Check(i) => {
                if let Some(c) = self.checks.get_mut(i) {
                    c.value = !c.value;
                }
                None
            }
            Message::DontAsk => {
                self.dont_ask = !self.dont_ask;
                None
            }
            Message::Submit => Some(Outcome::Submitted {
                purpose: self.purpose.clone(),
                values: self
                    .fields
                    .iter()
                    .map(|f| f.value.trim().to_owned())
                    .collect(),
                checks: self.checks.iter().map(|c| c.value).collect(),
                dont_ask_again: self.remember.clone().filter(|_| self.dont_ask),
            }),
            Message::Cancel => Some(Outcome::Cancelled {
                purpose: self.purpose.clone(),
            }),
        }
    }

    /// Submit triggered by Enter: ignored for deletions.
    pub fn enter(&mut self) -> Option<Outcome<P>> {
        if self.severity == Severity::Delete {
            None
        } else {
            self.update(Message::Submit)
        }
    }

    pub fn view<'a>(&'a self, t: &'a Tokens) -> Element<'a, Message> {
        let danger = self.severity == Severity::Delete;
        let accent = self.severity.accent(t);
        let mut col = Column::new().spacing(6).push(colored(
            t,
            format!("┌─ {}{} ", self.severity.glyph(), self.title),
            accent,
        ));
        for line in &self.body {
            col = col.push(mono(t, format!("│ {line}")));
        }
        for (i, f) in self.fields.iter().enumerate() {
            let mut input = text_input(&f.placeholder, &f.value)
                .on_input(move |v| Message::Field(i, v))
                .size(t.font_size)
                .padding(4)
                .style(style::field_input(t));
            if let Some(id) = FIELD_IDS.get(i) {
                input = input.id(*id);
            }
            if !danger {
                input = input.on_submit(Message::Submit);
            }
            col = col.push(column![dim(t, format!("│ {}", f.label)), input].spacing(2));
        }
        for (i, c) in self.checks.iter().enumerate() {
            let label = format!("{} {}", if c.value { "■" } else { "□" }, c.label);
            col = col.push(widgets::text_button(
                t,
                &label,
                None,
                Some(Message::Check(i)),
            ));
        }
        if self.remember.is_some() {
            let label = format!(
                "{} don't ask again (reset with :confirm reset)",
                if self.dont_ask { "■" } else { "□" }
            );
            col = col.push(widgets::text_button(
                t,
                &label,
                None,
                Some(Message::DontAsk),
            ));
        }
        if let Some(e) = &self.error {
            col = col.push(colored(t, format!("│ ⚠ {e}"), t.error()));
        }
        let confirm = if danger {
            widgets::danger_button(t, &self.confirm_label, Some(Message::Submit))
        } else {
            widgets::text_button(t, &self.confirm_label, Some("⏎"), Some(Message::Submit))
        };
        col = col.push(
            row![
                space().width(Length::Fill),
                widgets::text_button(t, "Cancel", Some("Esc"), Some(Message::Cancel)),
                confirm,
            ]
            .spacing(6),
        );
        if danger {
            col = col.push(dim(
                t,
                "└ this cannot be undone — click the red button to proceed",
            ));
        }

        container(col)
            .padding(Padding::from([10, 14]))
            .max_width(640)
            .style(move |_| container::Style {
                text_color: Some(t.fg),
                background: Some(Background::Color(t.surface)),
                border: Border {
                    color: accent,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_returns_trimmed_values_and_checks() {
        let mut d = Dialog::confirm("New branch", vec![], "Create", 7)
            .field("name", "", "feature/…")
            .check("switch to it", true);
        assert!(d.update(Message::Field(0, "  feat/x ".into())).is_none());
        let _ = d.update(Message::Check(0));
        match d.update(Message::Submit) {
            Some(Outcome::Submitted {
                purpose,
                values,
                checks,
                dont_ask_again,
            }) => {
                assert_eq!(purpose, 7);
                assert_eq!(values, ["feat/x"]);
                assert_eq!(checks, [false]);
                assert_eq!(dont_ask_again, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn enter_never_confirms_deletions() {
        let mut d = Dialog::confirm("Delete", vec![], "Delete", ()).severity(Severity::Delete);
        assert!(d.enter().is_none());
        let mut d = Dialog::confirm("Pull", vec![], "Pull", ()).severity(Severity::Change);
        assert!(d.enter().is_some());
    }

    #[test]
    fn dont_ask_again_only_for_changes() {
        let d = Dialog::confirm("x", vec![], "x", ())
            .severity(Severity::Delete)
            .remember_as("k");
        assert!(d.remember.is_none(), "deletions always ask");

        let mut d = Dialog::confirm("x", vec![], "x", ())
            .severity(Severity::Change)
            .remember_as("pull");
        let _ = d.update(Message::DontAsk);
        assert!(matches!(
            d.update(Message::Submit),
            Some(Outcome::Submitted { dont_ask_again: Some(k), .. }) if k == "pull"
        ));
    }

    #[test]
    fn cancel_returns_the_purpose() {
        let mut d = Dialog::confirm("x", vec![], "x", 3);
        assert!(matches!(
            d.update(Message::Cancel),
            Some(Outcome::Cancelled { purpose: 3 })
        ));
    }
}
