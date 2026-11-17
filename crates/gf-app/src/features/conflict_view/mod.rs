//! Conflict resolver for one file: each conflict block shows "ours" and
//! "theirs" (and the base with diff3) with buttons to keep one side, both,
//! or whole-file shortcuts. Applying writes the file and stages it once no
//! conflict is left. Writing is a change, so the app confirms it.

use std::path::PathBuf;

use gf_git::Side;
use gf_git::conflict::{Block, Choice, Segment};
use iced::widget::text::Wrapping;
use iced::widget::{Column, column, container, row, space};
use iced::{Background, Border, Color, Element, Length, Padding};

use crate::theme::{Tokens, style};
use crate::widgets::{self, colored, dim};

/// Unchanged lines shown around conflicts; longer runs are folded.
const CONTEXT_LINES: usize = 3;

#[derive(Debug, Clone)]
pub struct State {
    pub path: PathBuf,
    segments: Option<Vec<Segment>>,
    choices: Vec<Option<Choice>>,
    loading: bool,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Choose(usize, Choice),
    Apply,
    TakeFile(Side),
    MarkResolved,
    Close,
}

#[derive(Debug, Clone)]
pub enum Output {
    Apply {
        path: PathBuf,
        segments: Vec<Segment>,
        choices: Vec<Option<Choice>>,
    },
    TakeFile(PathBuf, Side),
    /// Stage the file as it is (resolved by hand in an editor).
    MarkResolved(PathBuf),
    Close,
}

impl State {
    pub fn loading(path: PathBuf) -> Self {
        Self {
            path,
            segments: None,
            choices: Vec::new(),
            loading: true,
            error: None,
        }
    }

    pub fn loaded(&mut self, path: &PathBuf, result: Result<Option<Vec<Segment>>, String>) {
        if &self.path != path {
            return;
        }
        self.loading = false;
        match result {
            Ok(Some(segments)) => {
                // Refreshes reload the file (the watcher fires often); the
                // choices made so far stay unless the file really changed.
                if self.segments.as_ref() != Some(&segments) {
                    let n = gf_git::conflict::conflict_count(&segments);
                    self.choices = vec![None; n];
                    self.segments = Some(segments);
                }
                self.error = None;
            }
            Ok(None) => {
                self.segments = None;
                self.error = Some("no conflict markers found (binary file, or already edited) — take a side or mark it resolved".into());
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn chosen(&self) -> usize {
        self.choices.iter().filter(|c| c.is_some()).count()
    }

    pub fn update(&mut self, message: Message) -> Option<Output> {
        match message {
            Message::Choose(i, c) => {
                if let Some(slot) = self.choices.get_mut(i) {
                    // Clicking the active choice again clears it.
                    *slot = if *slot == Some(c) { None } else { Some(c) };
                }
                None
            }
            Message::Apply => self.segments.clone().map(|segments| Output::Apply {
                path: self.path.clone(),
                segments,
                choices: self.choices.clone(),
            }),
            Message::TakeFile(side) => Some(Output::TakeFile(self.path.clone(), side)),
            Message::MarkResolved => Some(Output::MarkResolved(self.path.clone())),
            Message::Close => Some(Output::Close),
        }
    }

    fn block_view<'a>(&'a self, t: &'a Tokens, n: usize, b: &'a Block) -> Element<'a, Message> {
        let choice = self.choices.get(n).copied().flatten();
        let side = |label: String, text: &'a str, color: Color, picked: bool| {
            let mut col = Column::new().push(colored(t, label, color));
            for line in text.lines() {
                col = col.push(
                    widgets::mono(t, format!("  {}", widgets::format::code_line(line)))
                        .wrapping(Wrapping::None),
                );
            }
            if text.is_empty() {
                col = col.push(dim(t, "  (nothing)"));
            }
            container(col)
                .padding(Padding::from([2, 6]))
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(Background::Color(style::tint(
                        t.bg,
                        color,
                        if picked { 0.25 } else { 0.08 },
                    ))),
                    border: Border {
                        color: if picked { color } else { Color::TRANSPARENT },
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..container::Style::default()
                })
        };
        let ours_picked = matches!(
            choice,
            Some(Choice::Ours | Choice::Both | Choice::BothTheirsFirst)
        );
        let theirs_picked = matches!(
            choice,
            Some(Choice::Theirs | Choice::Both | Choice::BothTheirsFirst)
        );
        let mut sides = row![
            side(
                format!("◀ ours · {}", b.ours_label),
                &b.ours,
                t.added(),
                ours_picked
            ),
            side(
                format!("theirs · {} ▶", b.theirs_label),
                &b.theirs,
                t.remote(),
                theirs_picked
            ),
        ]
        .spacing(4);
        if let Some(base) = &b.base {
            sides = sides.push(side(
                "base (before both)".into(),
                base,
                t.dim,
                choice == Some(Choice::Base),
            ));
        }
        let pick = |label: &str, c: Choice| {
            let active = choice == Some(c);
            widgets::text_button(
                t,
                &if active {
                    format!("■ {label}")
                } else {
                    label.to_owned()
                },
                None,
                Some(Message::Choose(n, c)),
            )
        };
        let mut buttons = row![
            colored(t, format!("conflict {}", n + 1), t.warning()),
            space().width(Length::Fill),
            pick("ours", Choice::Ours),
            pick("theirs", Choice::Theirs),
            pick("both", Choice::Both),
            pick("both (theirs first)", Choice::BothTheirsFirst),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);
        if b.base.is_some() {
            buttons = buttons.push(pick("base", Choice::Base));
        }
        column![buttons, sides].spacing(2).into()
    }

    pub fn view<'a>(&'a self, t: &'a Tokens) -> Element<'a, Message> {
        let accent = t.warning();
        let total = self.choices.len();
        let header = row![
            colored(
                t,
                format!(
                    "{} · {}/{} chosen",
                    self.path.display(),
                    self.chosen(),
                    total
                ),
                t.fg
            )
            .wrapping(Wrapping::None)
            .width(Length::Fill),
            widgets::tip(
                t,
                widgets::text_button(
                    t,
                    "whole file: ours",
                    None,
                    Some(Message::TakeFile(Side::Ours))
                ),
                "keep your version of the entire file".into()
            ),
            widgets::tip(
                t,
                widgets::text_button(
                    t,
                    "whole file: theirs",
                    None,
                    Some(Message::TakeFile(Side::Theirs))
                ),
                "keep the incoming version of the entire file".into()
            ),
            widgets::tip(
                t,
                widgets::text_button(t, "mark resolved", None, Some(Message::MarkResolved)),
                "stage the file as it is now (after editing it yourself)".into()
            ),
            widgets::icon_button(t, "✕", t.dim, "close", Message::Close),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);

        let body: Element<'a, Message> = if self.loading {
            widgets::empty_hint(t, "reading conflicts…")
        } else if let Some(e) = &self.error {
            colored(t, format!("  {e}"), t.warning()).into()
        } else if let Some(segments) = &self.segments {
            let mut col = Column::new().spacing(6);
            let mut n = 0;
            for s in segments {
                match s {
                    Segment::Common(text) => {
                        let lines: Vec<&str> = text.lines().collect();
                        let fold = lines.len() > CONTEXT_LINES * 2 + 1;
                        let mut c = Column::new();
                        let shown: Vec<&str> = if fold {
                            lines[..CONTEXT_LINES]
                                .iter()
                                .chain(lines[lines.len() - CONTEXT_LINES..].iter())
                                .copied()
                                .collect()
                        } else {
                            lines.clone()
                        };
                        for (i, line) in shown.iter().enumerate() {
                            if fold && i == CONTEXT_LINES {
                                c = c.push(dim(
                                    t,
                                    format!(
                                        "  … {} unchanged lines",
                                        lines.len() - CONTEXT_LINES * 2
                                    ),
                                ));
                            }
                            c = c.push(dim(t, format!("  {line}")).wrapping(Wrapping::None));
                        }
                        col = col.push(c);
                    }
                    Segment::Conflict(b) => {
                        col = col.push(self.block_view(t, n, b));
                        n += 1;
                    }
                }
            }
            widgets::scroll(col)
        } else {
            widgets::empty_hint(t, "nothing to show")
        };

        let footer = row![
            dim(
                t,
                if self.chosen() == total && total > 0 {
                    "all conflicts chosen — apply writes the file and stages it"
                } else {
                    "unchosen conflicts keep their markers when applied"
                }
            )
            .width(Length::Fill),
            widgets::text_button(
                t,
                "apply choices",
                None,
                (self.chosen() > 0).then_some(Message::Apply)
            ),
        ]
        .align_y(iced::Alignment::Center);

        widgets::accent_panel(
            t,
            "RESOLVE CONFLICT".into(),
            accent,
            column![header, body, footer].spacing(4),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        let text = "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> topic\nb\n";
        let mut s = State::loading("f.txt".into());
        s.loaded(&"f.txt".into(), Ok(gf_git::conflict::parse(text)));
        s
    }

    #[test]
    fn choose_toggles_and_apply_carries_choices() {
        let mut s = state();
        assert_eq!(s.choices, vec![None]);
        let _ = s.update(Message::Choose(0, Choice::Theirs));
        assert_eq!(s.chosen(), 1);
        match s.update(Message::Apply) {
            Some(Output::Apply { choices, .. }) => assert_eq!(choices, vec![Some(Choice::Theirs)]),
            other => panic!("unexpected {other:?}"),
        }
        let _ = s.update(Message::Choose(0, Choice::Theirs));
        assert_eq!(s.chosen(), 0, "clicking again clears");
    }

    #[test]
    fn reloading_the_same_content_keeps_choices() {
        let mut s = state();
        let _ = s.update(Message::Choose(0, Choice::Ours));
        // A refresh reloads the file: same text, choices stay.
        let same = "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> topic\nb\n";
        s.loaded(&"f.txt".into(), Ok(gf_git::conflict::parse(same)));
        assert_eq!(s.choices, vec![Some(Choice::Ours)]);
        // The file changed on disk: choices no longer apply.
        let other = "a\n<<<<<<< HEAD\nx2\n=======\ny\n>>>>>>> topic\nb\n";
        s.loaded(&"f.txt".into(), Ok(gf_git::conflict::parse(other)));
        assert_eq!(s.choices, vec![None]);
    }

    #[test]
    fn unparseable_file_explains_what_to_do() {
        let mut s = State::loading("bin".into());
        s.loaded(&"bin".into(), Ok(None));
        assert!(
            s.error
                .as_deref()
                .is_some_and(|e| e.contains("take a side"))
        );
    }
}
