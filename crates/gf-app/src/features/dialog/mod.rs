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
//! | Failure  | red, `✗`        | dismisses      | no                |
//!
//! [`Dialog::notice`] builds a dialog with a single `[ OK ⏎ ]` button (no
//! Cancel), used to report a failure and what to do about it.
//!
//! Generic over the purpose `P`, which the app uses to know what to do on
//! submit. Apps should not build confirmations by hand: use the app's
//! shared `confirm` helper so every delete/change asks the same way.

use iced::widget::{Column, column, container, row, space, text_editor, text_input};
use iced::{Background, Border, Color, Element, Length, Padding};

use crate::theme::{Tokens, style};
use crate::widgets::{self, colored, dim, mono};

/// Id of the multi-line text area.
pub const AREA_ID: &str = "gf-dialog-area";

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
    /// Reports something that went wrong; nothing runs on confirm.
    Failure,
}

impl Severity {
    fn accent(self, t: &Tokens) -> Color {
        match self {
            Self::Normal => t.accent(),
            Self::Change => t.warning(),
            Self::Delete | Self::Failure => t.error(),
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Normal => "",
            Self::Change => "? ",
            Self::Delete => "⚠ ",
            Self::Failure => "✗ ",
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

/// A multi-line text box below the fields. Its text is handed back after
/// the fields' values (see [`Outcome::Submitted`]).
pub struct Area {
    pub label: String,
    pub placeholder: String,
    pub content: text_editor::Content,
}

impl Clone for Area {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            placeholder: self.placeholder.clone(),
            content: text_editor::Content::with_text(&self.content.text()),
        }
    }
}

impl std::fmt::Debug for Area {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Area")
            .field("label", &self.label)
            .field("text", &self.content.text())
            .finish()
    }
}

/// One of several options (radio buttons). An option marked dangerous
/// turns the dialog into a deletion while it is selected.
#[derive(Debug, Clone)]
pub struct Choice {
    pub label: String,
    pub options: Vec<(String, bool)>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct Dialog<P> {
    pub title: String,
    pub body: Vec<String>,
    pub fields: Vec<Field>,
    pub area: Option<Area>,
    pub checks: Vec<Check>,
    pub choice: Option<Choice>,
    pub confirm_label: String,
    pub severity: Severity,
    pub purpose: P,
    /// Validation message shown above the buttons.
    pub error: Option<String>,
    /// Key of a skippable confirmation; shows a "don't ask again" box.
    pub remember: Option<String>,
    dont_ask: bool,
    /// Only one button (no Cancel): the dialog just informs.
    notice: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Field(usize, String),
    Area(text_editor::Action),
    Check(usize),
    Choose(usize),
    DontAsk,
    Submit,
    Cancel,
}

/// What the user decided.
#[derive(Debug, Clone)]
pub enum Outcome<P> {
    Submitted {
        purpose: P,
        /// The fields' values (trimmed), then the text area's text (with
        /// trailing whitespace removed) when the dialog has one.
        values: Vec<String>,
        checks: Vec<bool>,
        /// Selected option of the choice, if the dialog has one.
        choice: Option<usize>,
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
            area: None,
            checks: Vec::new(),
            choice: None,
            confirm_label: confirm_label.into(),
            severity: Severity::Normal,
            purpose,
            error: None,
            remember: None,
            dont_ask: false,
            notice: false,
        }
    }

    /// A failure report with a single `[ OK ⏎ ]` button; Enter and Esc
    /// both dismiss it.
    pub fn notice(title: impl Into<String>, body: Vec<String>, purpose: P) -> Self {
        Self {
            notice: true,
            ..Self::confirm(title, body, "OK", purpose).severity(Severity::Failure)
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

    /// A multi-line text box (one per dialog), shown after the fields.
    pub fn area(
        mut self,
        label: impl Into<String>,
        text: &str,
        placeholder: impl Into<String>,
    ) -> Self {
        self.area = Some(Area {
            label: label.into(),
            placeholder: placeholder.into(),
            content: text_editor::Content::with_text(text),
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

    /// Radio options; `(label, dangerous)` each, `selected` first.
    pub fn choice(
        mut self,
        label: impl Into<String>,
        options: Vec<(String, bool)>,
        selected: usize,
    ) -> Self {
        self.choice = Some(Choice {
            label: label.into(),
            options,
            selected,
        });
        self
    }

    /// The severity in effect: a dangerous option makes it a deletion.
    fn effective_severity(&self) -> Severity {
        let dangerous = self
            .choice
            .as_ref()
            .and_then(|c| c.options.get(c.selected))
            .is_some_and(|(_, d)| *d);
        if dangerous {
            Severity::Delete
        } else {
            self.severity
        }
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
            Message::Area(action) => {
                if let Some(a) = self.area.as_mut() {
                    a.content.perform(action);
                }
                None
            }
            Message::Check(i) => {
                if let Some(c) = self.checks.get_mut(i) {
                    c.value = !c.value;
                }
                None
            }
            Message::Choose(i) => {
                if let Some(c) = self.choice.as_mut().filter(|c| i < c.options.len()) {
                    c.selected = i;
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
                    .chain(
                        self.area
                            .as_ref()
                            .map(|a| a.content.text().trim_end().to_owned()),
                    )
                    .collect(),
                checks: self.checks.iter().map(|c| c.value).collect(),
                choice: self.choice.as_ref().map(|c| c.selected),
                dont_ask_again: self.remember.clone().filter(|_| self.dont_ask),
            }),
            Message::Cancel => Some(Outcome::Cancelled {
                purpose: self.purpose.clone(),
            }),
        }
    }

    /// Submit triggered by Enter: ignored for deletions.
    pub fn enter(&mut self) -> Option<Outcome<P>> {
        if self.effective_severity() == Severity::Delete {
            None
        } else {
            self.update(Message::Submit)
        }
    }

    pub fn view<'a>(&'a self, t: &'a Tokens) -> Element<'a, Message> {
        let severity = self.effective_severity();
        let danger = severity == Severity::Delete;
        let accent = severity.accent(t);
        let mut col = Column::new().spacing(6).push(colored(
            t,
            format!("┌─ {}{} ", severity.glyph(), self.title),
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
        if let Some(area) = &self.area {
            let editor = text_editor(&area.content)
                .id(AREA_ID)
                .placeholder(area.placeholder.as_str())
                .on_action(Message::Area)
                .size(t.font_size)
                .height(Length::Fixed(t.font_size * 9.0))
                .style(style::text_area(t));
            col = col.push(column![dim(t, format!("│ {}", area.label)), editor].spacing(2));
        }
        if let Some(choice) = &self.choice {
            col = col.push(dim(t, format!("│ {}", choice.label)));
            for (i, (label, dangerous)) in choice.options.iter().enumerate() {
                let picked = i == choice.selected;
                let text = format!("{} {label}", if picked { "(•)" } else { "( )" });
                col = col.push(if *dangerous && picked {
                    widgets::danger_button(t, &text, Some(Message::Choose(i)))
                } else {
                    widgets::text_button(t, &text, None, Some(Message::Choose(i)))
                });
            }
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
        let mut buttons = row![space().width(Length::Fill)].spacing(6);
        if !self.notice {
            buttons = buttons.push(widgets::text_button(
                t,
                "Cancel",
                Some("Esc"),
                Some(Message::Cancel),
            ));
        }
        col = col.push(buttons.push(confirm));
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
                ..
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
    fn dangerous_choice_turns_the_dialog_into_a_deletion() {
        let mut d = Dialog::confirm("Reset", vec![], "Reset", ())
            .severity(Severity::Change)
            .choice(
                "mode",
                vec![("soft".into(), false), ("hard".into(), true)],
                0,
            );
        assert_eq!(d.effective_severity(), Severity::Change);
        let _ = d.update(Message::Choose(1));
        assert_eq!(d.effective_severity(), Severity::Delete);
        assert!(d.enter().is_none(), "Enter never runs the dangerous option");
        assert!(matches!(
            d.update(Message::Submit),
            Some(Outcome::Submitted {
                choice: Some(1),
                ..
            })
        ));
        let _ = d.update(Message::Choose(9));
        assert_eq!(
            d.choice.as_ref().map(|c| c.selected),
            Some(1),
            "out of range ignored"
        );
    }

    #[test]
    fn area_text_comes_after_the_fields() {
        let mut d = Dialog::confirm("Squash", vec![], "Squash", ())
            .field("summary", " feat ", "")
            .area("details", "line 1\nline 2\n\n", "");
        let _ = d.update(Message::Area(text_editor::Action::Move(
            text_editor::Motion::DocumentEnd,
        )));
        match d.update(Message::Submit) {
            Some(Outcome::Submitted { values, .. }) => {
                assert_eq!(values, ["feat", "line 1\nline 2"]);
            }
            other => panic!("unexpected {other:?}"),
        }
        let copy = d.clone();
        assert_eq!(
            copy.area.map(|a| a.content.text()),
            Some("line 1\nline 2\n\n".to_owned())
        );
    }

    #[test]
    fn notice_is_dismissed_with_enter() {
        let mut d = Dialog::notice("Push rejected", vec!["why".into()], 1);
        assert_eq!(d.effective_severity(), Severity::Failure);
        assert!(d.notice);
        assert!(matches!(
            d.enter(),
            Some(Outcome::Submitted { purpose: 1, .. })
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
