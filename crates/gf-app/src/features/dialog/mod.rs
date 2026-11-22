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
use iced::{Background, Border, Color, Element, Length, Padding, Task};

use crate::nav::{self, Key};
use crate::theme::{Tokens, style};
use crate::widgets::virtual_list::{ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim, mono};

/// Id of the multi-line text area.
pub const AREA_ID: &str = "gf-dialog-area";

/// Id of the first text field, focused when the dialog opens.
pub const FIRST_FIELD_ID: &str = "gf-dialog-field-0";

const REVIEW_ID: &str = "gf-dialog-review";

/// Widest the box gets, and its padding left and right.
const MAX_WIDTH: f32 = 640.0;
const PAD_X: f32 = 14.0;
const MAX_REVIEW_ROWS: usize = 10;

fn review_row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.5).round()
}

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
    pub options: Vec<Opt>,
    pub selected: usize,
    /// Drawn as a row of action buttons instead of radio buttons: one
    /// click runs an option, ⏎ runs the selected one, and only the
    /// option under the pointer (else the selected one) shows its lines.
    pub inline: bool,
    /// Option under the pointer (inline only).
    hovered: Option<usize>,
}

/// How a line under an option reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Hint,
    /// Why the option cannot run.
    Problem,
}

/// One option of a [`Choice`].
#[derive(Debug, Clone)]
pub struct Opt {
    pub label: String,
    pub dangerous: bool,
    /// Pressing it picks the option and runs it (a dangerous one is only
    /// picked: it still needs a click).
    pub key: Option<char>,
    /// Shown under the label: hints and problems.
    pub lines: Vec<(String, Tone)>,
    /// Cannot be picked (the lines say why).
    pub disabled: bool,
}

impl Opt {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            dangerous: false,
            key: None,
            lines: Vec::new(),
            disabled: false,
        }
    }

    pub fn key(mut self, key: char) -> Self {
        self.key = Some(key);
        self
    }

    pub fn line(mut self, text: impl Into<String>, tone: Tone) -> Self {
        self.lines.push((text.into(), tone));
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

#[derive(Debug, Clone)]
struct Review {
    entries: Vec<String>,
    widest: usize,
    scroll: ScrollPos,
}

#[derive(Debug, Clone)]
pub struct Dialog<P> {
    pub title: String,
    pub body: Vec<String>,
    review: Option<Review>,
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
    /// The action can be undone with ⌘Z (said instead of "cannot be
    /// undone" under a deletion).
    undoable: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Field(usize, String),
    Area(text_editor::Action),
    ReviewScrolled(ScrollPos),
    Check(usize),
    Choose(usize),
    /// Inline option clicked: picks it and runs it.
    Pick(usize),
    /// Pointer over an inline option (`None`: left it).
    Hover(Option<usize>),
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
            review: None,
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
            undoable: false,
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

    /// A scrollable list of the exact items affected by this confirmation.
    pub fn review(mut self, entries: Vec<String>) -> Self {
        if let Some(widest) = entries.iter().map(|line| columns(line)).max() {
            self.review = Some(Review {
                entries,
                widest,
                scroll: ScrollPos::default(),
            });
        }
        self
    }

    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// The action can be undone (⌘Z): the footer says so.
    pub fn undoable(mut self) -> Self {
        self.undoable = true;
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
        self,
        label: impl Into<String>,
        options: Vec<(String, bool)>,
        selected: usize,
    ) -> Self {
        let options = options
            .into_iter()
            .map(|(label, dangerous)| Opt {
                dangerous,
                ..Opt::new(label)
            })
            .collect();
        self.options(label, options, selected)
    }

    /// Options with keys and lines (see [`Opt`]); `selected` first.
    pub fn options(mut self, label: impl Into<String>, options: Vec<Opt>, selected: usize) -> Self {
        self.choice = Some(Choice {
            label: label.into(),
            options,
            selected,
            inline: false,
            hovered: None,
        });
        self
    }

    /// Options as action buttons in place of the confirm button (see
    /// [`Choice::inline`]); `selected` runs on ⏎.
    pub fn actions(mut self, options: Vec<Opt>, selected: usize) -> Self {
        self = self.options("", options, selected);
        if let Some(c) = self.choice.as_mut() {
            c.inline = true;
        }
        self
    }

    /// A letter key: picks the option it belongs to and runs it (only
    /// picks a dangerous one). `None` when no enabled option has the key.
    pub fn key(&mut self, key: char) -> Option<Outcome<P>> {
        let i = self.choice.as_ref().and_then(|c| {
            c.options
                .iter()
                .position(|o| o.key == Some(key) && !o.disabled)
        })?;
        let _ = self.update(Message::Choose(i));
        self.enter()
    }

    /// Whether `key` belongs to an option.
    pub fn has_key(&self, key: char) -> bool {
        self.choice
            .as_ref()
            .is_some_and(|c| c.options.iter().any(|o| o.key == Some(key)))
    }

    /// The severity in effect: a dangerous option makes it a deletion.
    fn effective_severity(&self) -> Severity {
        let dangerous = self
            .choice
            .as_ref()
            .and_then(|c| c.options.get(c.selected))
            .is_some_and(|o| o.dangerous);
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
            Message::ReviewScrolled(scroll) => {
                if let Some(review) = self.review.as_mut() {
                    review.scroll = scroll;
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
                let enabled = |c: &&mut Choice| c.options.get(i).is_some_and(|o| !o.disabled);
                if let Some(c) = self.choice.as_mut().filter(enabled) {
                    c.selected = i;
                }
                None
            }
            Message::Pick(i) => {
                let enabled = self
                    .choice
                    .as_ref()
                    .and_then(|c| c.options.get(i))
                    .is_some_and(|o| !o.disabled);
                if !enabled {
                    return None;
                }
                let _ = self.update(Message::Choose(i));
                self.update(Message::Submit)
            }
            Message::Hover(i) => {
                if let Some(c) = self.choice.as_mut() {
                    c.hovered = i;
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

    /// Lets a keyboard user inspect a long confirmation list.
    pub fn review_nav(&self, key: Key, t: &Tokens) -> Option<Task<Message>> {
        let review = self.review.as_ref()?;
        let height = review_row_height(t);
        let page = if review.scroll.height > height {
            review.scroll.height - height
        } else {
            height * 10.0
        };
        Some(match key {
            Key::Up => nav::scroll_by(REVIEW_ID, -height),
            Key::Down => nav::scroll_by(REVIEW_ID, height),
            Key::PageUp => nav::scroll_by(REVIEW_ID, -page),
            Key::PageDown => nav::scroll_by(REVIEW_ID, page),
            Key::Home => nav::scroll_y(REVIEW_ID, 0.0),
            Key::End => iced::widget::operation::snap_to_end(REVIEW_ID),
            _ => return None,
        })
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
        if let Some(review) = &self.review {
            let row_height = review_row_height(t);
            let rows =
                u16::try_from(review.entries.len().min(MAX_REVIEW_ROWS)).expect("at most 10 rows");
            let list = virtual_list(
                REVIEW_ID,
                review.entries.len(),
                row_height,
                t.cols(review.widest + 3),
                review.scroll,
                |i| {
                    mono(t, review.entries[i].as_str())
                        .wrapping(iced::widget::text::Wrapping::None)
                        .into()
                },
                Message::ReviewScrolled,
            );
            col = col
                .push(container(list).height(Length::Fixed(row_height * (f32::from(rows) + 1.0))));
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
        let inline = self.choice.as_ref().filter(|c| c.inline);
        if let Some(choice) = self.choice.as_ref().filter(|c| !c.inline) {
            col = col.push(dim(t, format!("│ {}", choice.label)));
            for (i, o) in choice.options.iter().enumerate() {
                let picked = i == choice.selected;
                let key = o.key.map(|k| format!("[{k}] ")).unwrap_or_default();
                let text = format!("{} {key}{}", if picked { "(•)" } else { "( )" }, o.label);
                let pick = (!o.disabled).then_some(Message::Choose(i));
                col = col.push(if o.dangerous && picked {
                    widgets::danger_button(t, &text, pick)
                } else {
                    widgets::text_button(t, &text, None, pick)
                });
                for (line, tone) in &o.lines {
                    let color = match tone {
                        Tone::Hint => t.dim,
                        Tone::Problem => t.error(),
                    };
                    col = col.push(colored(t, format!("│       {line}"), color));
                }
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
        if let Some(choice) = inline {
            // Full width (up to the cap) whatever the hint says, so
            // hovering cannot resize the box.
            let content = col.push(inline_actions(t, choice)).width(Length::Fill);
            return self.frame(t, accent, content);
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
                if self.undoable {
                    "└ ⌘Z undoes it afterwards — click the red button to proceed"
                } else {
                    "└ this cannot be undone — click the red button to proceed"
                },
            ));
        }

        self.frame(t, accent, col)
    }

    /// The bordered box around the dialog's content.
    fn frame<'a>(
        &'a self,
        t: &'a Tokens,
        accent: Color,
        col: Column<'a, Message>,
    ) -> Element<'a, Message> {
        container(col)
            .padding(Padding::from([10.0, PAD_X]))
            .max_width(MAX_WIDTH)
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

/// Inline options: the lines of the option under the pointer (else the
/// selected one) and why any disabled option cannot run, then the last
/// row: a button per option (the selected one marked ⏎) and Cancel.
fn inline_actions<'a>(t: &'a Tokens, choice: &'a Choice) -> Element<'a, Message> {
    let mut buttons = row![].spacing(6);
    for (i, o) in choice.options.iter().enumerate() {
        let keys = match (o.key, i == choice.selected) {
            (Some(k), true) => format!("{k} ⏎"),
            (Some(k), false) => k.to_string(),
            (None, true) => "⏎".into(),
            (None, false) => String::new(),
        };
        let label = if o.disabled {
            format!("{} ✗", o.label)
        } else {
            o.label.clone()
        };
        let pick = (!o.disabled).then_some(Message::Pick(i));
        let shortcut = (!keys.is_empty()).then_some(keys.as_str());
        let button = if o.dangerous {
            widgets::danger_button(t, &label, pick)
        } else {
            widgets::text_button(t, &label, shortcut, pick)
        };
        buttons = buttons.push(
            iced::widget::mouse_area(button)
                .on_enter(Message::Hover(Some(i)))
                .on_exit(Message::Hover(None)),
        );
    }
    buttons = buttons
        .push(space().width(Length::Fill))
        .push(widgets::text_button(
            t,
            "Cancel",
            Some("Esc"),
            Some(Message::Cancel),
        ));
    let shown = choice
        .hovered
        .filter(|i| choice.options.get(*i).is_some())
        .unwrap_or(choice.selected);
    let mut col = Column::new().spacing(6);
    for o in &choice.options {
        for (line, tone) in &o.lines {
            if *tone == Tone::Problem {
                col = col.push(one_line(t, format!("│ {line}"), t.error()));
            }
        }
    }
    // Hovering swaps the hints in place: as many rows as the option with
    // the most, one line each, so the buttons never move under the pointer
    // (moving would end the hover and start it again).
    let hints = |o: &'a Opt| o.lines.iter().filter(|(_, tone)| *tone == Tone::Hint);
    let rows = choice
        .options
        .iter()
        .map(|o| hints(o).count())
        .max()
        .unwrap_or(0);
    let mut shown_hints = choice.options.get(shown).map(|o| hints(o));
    for _ in 0..rows {
        let line = shown_hints
            .as_mut()
            .and_then(Iterator::next)
            .map_or(String::from("│"), |(line, _)| format!("│ {line}"));
        col = col.push(one_line(t, line, t.dim));
    }
    col.push(buttons).into()
}

/// Columns of text that fit inside the widest box.
fn inner_cols(t: &Tokens) -> usize {
    let cols = ((MAX_WIDTH - 2.0 * PAD_X) / t.char_width()).floor();
    // Truncation toward zero is the point; a negative width means none.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let cols = cols.max(0.0) as usize;
    // One spare column: glyphs like ⌘ run a little wider than the estimate.
    cols.saturating_sub(1)
}

/// One line that never wraps: cut with `…` to the box's width, the whole
/// text shown on hover when cut.
fn one_line<'a>(t: &'a Tokens, line: String, color: Color) -> Element<'a, Message> {
    let fit = widgets::format::truncate_columns(&line, inner_cols(t));
    let cut = fit != line;
    let text = colored(t, fit, color).wrapping(iced::widget::text::Wrapping::None);
    if cut {
        let full = line.trim_start_matches(['│', ' ']).to_owned();
        widgets::tip_at_cursor(t, text, full)
    } else {
        text.into()
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
    fn option_keys_pick_and_run_but_skip_disabled_options() {
        let opts = || {
            vec![
                Opt::new("stash, then switch").key('s'),
                Opt::new("carry changes over").key('c').disabled(true),
            ]
        };
        let mut d = Dialog::confirm("Switch", vec![], "Switch", ())
            .severity(Severity::Change)
            .options("", opts(), 0);
        assert!(d.has_key('c') && !d.has_key('x'));
        assert!(d.key('c').is_none(), "disabled");
        let _ = d.update(Message::Choose(1));
        assert_eq!(
            d.choice.as_ref().map(|c| c.selected),
            Some(0),
            "not by click either"
        );
        assert!(matches!(
            d.key('s'),
            Some(Outcome::Submitted {
                choice: Some(0),
                ..
            })
        ));
    }

    #[test]
    fn inline_actions_run_on_click_but_not_when_disabled() {
        let mut d = Dialog::confirm("Switch", vec![], "Switch", ())
            .severity(Severity::Change)
            .actions(
                vec![
                    Opt::new("stash & switch").key('s'),
                    Opt::new("bring changes").key('c').disabled(true),
                ],
                0,
            );
        assert!(d.update(Message::Pick(1)).is_none(), "disabled");
        assert!(d.update(Message::Hover(Some(1))).is_none());
        assert_eq!(d.choice.as_ref().map(|c| c.selected), Some(0));
        assert!(matches!(
            d.update(Message::Pick(0)),
            Some(Outcome::Submitted {
                choice: Some(0),
                ..
            })
        ));
        assert!(matches!(
            d.enter(),
            Some(Outcome::Submitted {
                choice: Some(0),
                ..
            })
        ));
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
