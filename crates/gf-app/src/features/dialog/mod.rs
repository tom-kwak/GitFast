//! Modal dialog in TUI style: a bordered box with a title, explanation,
//! optional text fields and checkboxes, and `[ Cancel Esc ]` / `[ OK ⏎ ]`.
//!
//! Destructive dialogs (`danger`) are drawn in red and never confirm on a
//! bare Enter — the user has to click the red button. Generic over the
//! purpose `P`, which the app uses to know what to do on submit.

use iced::widget::{Column, column, container, row, space, text_input};
use iced::{Background, Border, Element, Length, Padding};

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
    pub danger: bool,
    pub purpose: P,
    /// Validation message shown above the buttons.
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Field(usize, String),
    Check(usize),
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
    },
    Cancelled,
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
            danger: false,
            purpose,
            error: None,
        }
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
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
            Message::Submit => Some(Outcome::Submitted {
                purpose: self.purpose.clone(),
                values: self
                    .fields
                    .iter()
                    .map(|f| f.value.trim().to_owned())
                    .collect(),
                checks: self.checks.iter().map(|c| c.value).collect(),
            }),
            Message::Cancel => Some(Outcome::Cancelled),
        }
    }

    /// Submit triggered by Enter: ignored for destructive dialogs.
    pub fn enter(&mut self) -> Option<Outcome<P>> {
        if self.danger {
            None
        } else {
            self.update(Message::Submit)
        }
    }

    pub fn view<'a>(&'a self, t: &'a Tokens) -> Element<'a, Message> {
        let accent = if self.danger { t.error() } else { t.accent() };
        let mut col =
            Column::new()
                .spacing(6)
                .push(colored(t, format!("┌─ {} ", self.title), accent));
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
            if !self.danger {
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
        if let Some(e) = &self.error {
            col = col.push(colored(t, format!("│ ⚠ {e}"), t.error()));
        }
        let confirm = if self.danger {
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
        if self.danger {
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
            }) => {
                assert_eq!(purpose, 7);
                assert_eq!(values, ["feat/x"]);
                assert_eq!(checks, [false]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn enter_never_confirms_danger() {
        let mut d = Dialog::confirm("Delete", vec![], "Delete", ()).danger();
        assert!(d.enter().is_none());
        let mut d = Dialog::confirm("Fetch", vec![], "OK", ());
        assert!(d.enter().is_some());
    }
}
