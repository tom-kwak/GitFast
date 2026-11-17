//! The undo history: every step gf can undo or redo, opened from the
//! status line (`▾`) or `:undo list`. Picking a step undoes (or redoes)
//! everything up to it, one step at a time, each checked as with ⌘Z.
//!
//! ```text
//!                         ┌─ UNDO HISTORY ─────── ⏎ go back to here · esc ┐
//!                         │   ↷ stage a.txt                       2m ago   │
//!                         │ ── now ── ⌘⇧Z redo · ⌘Z undo ──────────────── │
//!                         │ ▸ ↶ discard src/foo.rs                just now │
//!                         │   ↶ commit: Fix parser                3m ago   │
//!                         └──────────────────────────────────────────────── ┘
//! ```

use iced::widget::{Column, button, container, mouse_area, row, space, stack};
use iced::{Color, Element, Length, Padding};

use crate::theme::{Tokens, style};
use crate::undo::{History, age};
use crate::widgets::{colored, dim, format};

/// Rows shown at once (the newest ones; older steps are rarely wanted).
const ROWS: usize = 18;

#[derive(Debug, Clone)]
pub enum Message {
    /// The `▾` button in the status line.
    Toggle,
    Pick(usize),
    Hover(usize),
    Dismiss,
}

/// One step in the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// An undone step (redo brings it back).
    pub redo: bool,
    /// Steps before it on its side: picking it runs `depth + 1` steps.
    pub depth: usize,
    pub label: String,
    pub at: u64,
}

/// Redo steps (farthest first), then undo steps (newest first).
pub fn rows(h: &History) -> Vec<Row> {
    let redo = h.redo.iter().enumerate().map(|(i, s)| Row {
        redo: true,
        depth: h.redo.len() - 1 - i,
        label: s.label.clone(),
        at: s.at,
    });
    let undo = h.undo.iter().rev().enumerate().map(|(depth, s)| Row {
        redo: false,
        depth,
        label: s.label.clone(),
        at: s.at,
    });
    redo.chain(undo).collect()
}

#[derive(Debug, Clone, Default)]
pub struct State {
    /// Highlighted row; `None` = the newest undo step.
    pub selected: Option<usize>,
}

impl State {
    /// The highlighted row, defaulting to the step ⌘Z would undo.
    pub fn current(&self, rows: &[Row]) -> usize {
        self.selected
            .unwrap_or_else(|| rows.iter().position(|r| !r.redo).unwrap_or(0))
            .min(rows.len().saturating_sub(1))
    }

    pub fn move_by(&mut self, rows: &[Row], by: isize) {
        let cur = self.current(rows);
        self.selected = Some(
            cur.saturating_add_signed(by)
                .min(rows.len().saturating_sub(1)),
        );
    }

    pub fn view<'a>(&self, t: &'a Tokens, h: &History) -> Element<'a, Message> {
        let rows = rows(h);
        let selected = self.current(&rows);
        // Keep the highlighted row in the window.
        let first = selected
            .saturating_sub(ROWS - 1)
            .min(rows.len().saturating_sub(ROWS));
        let mut list = Column::new().spacing(1);
        let mut now_drawn = false;
        for (i, r) in rows.iter().enumerate().skip(first).take(ROWS) {
            if !r.redo && !now_drawn {
                now_drawn = true;
                if i > 0 {
                    list = list.push(dim(t, "── now ── ⌘⇧Z redo · ⌘Z undo ──"));
                }
            }
            let is_sel = i == selected;
            let (icon, color) = if r.redo {
                ("↷", t.dim)
            } else {
                ("↶", t.accent())
            };
            let line = row![
                colored(t, if is_sel { "▸" } else { " " }, t.accent()),
                colored(t, icon, color),
                colored(
                    t,
                    format::truncate(&r.label, 60),
                    if r.redo { t.dim } else { t.fg }
                )
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::None),
                dim(t, age(r.at)),
            ]
            .spacing(8);
            list = list.push(
                mouse_area(
                    button(line)
                        .width(Length::Fill)
                        .padding(Padding::from([2, 6]))
                        .style(style::row_button(t, is_sel))
                        .on_press(Message::Pick(i)),
                )
                .on_enter(Message::Hover(i)),
            );
        }
        if !now_drawn && !rows.is_empty() {
            list = list.push(dim(t, "── now ── nothing left to undo ──"));
        }
        let header = row![
            colored(t, "UNDO HISTORY", t.accent()),
            space().width(Length::Fill),
            dim(t, "⏎ go back to here · esc"),
        ]
        .spacing(8);
        let more = rows.len().saturating_sub(first + ROWS);
        let footer = dim(
            t,
            if more > 0 {
                format!("{more} older · undo also survives restarting gf")
            } else {
                "undo also survives restarting gf".to_owned()
            },
        );
        let panel = container(
            Column::with_children([header.into(), list.into(), footer.into()]).spacing(6),
        )
        .padding(Padding::from([6, 8]))
        .width(Length::Fill)
        .max_width(t.cols(80))
        .style(style::tooltip(t));
        // Right above the status line, flush with its right end.
        let placed = container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Bottom)
            .padding(Padding {
                top: 0.0,
                right: 8.0,
                bottom: (t.font_size * 2.2).round(),
                left: 0.0,
            });
        let backdrop = mouse_area(
            container(space())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Color::TRANSPARENT.into()),
                    ..container::Style::default()
                }),
        )
        .on_press(Message::Dismiss)
        .on_right_press(Message::Dismiss);
        stack![backdrop, placed].into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::undo::Step;
    use gf_git::{Capture, HeadMode, Scope};

    fn history(undo: &[&str], redo: &[&str]) -> History {
        let step = |l: &&str| {
            Step::new(
                (*l).into(),
                Scope::default(),
                Capture::default(),
                Capture::default(),
                HeadMode::Keep,
            )
        };
        History {
            undo: undo.iter().map(step).collect(),
            redo: redo.iter().map(step).collect(),
        }
    }

    #[test]
    fn redo_steps_come_first_then_newest_undo() {
        // `b` was undone last, so it is redone first (next to "now").
        let h = history(&["a1", "a2"], &["c", "b"]);
        let r = rows(&h);
        let shown: Vec<(&str, bool, usize)> = r
            .iter()
            .map(|r| (r.label.as_str(), r.redo, r.depth))
            .collect();
        assert_eq!(
            shown,
            [
                ("c", true, 1),
                ("b", true, 0),
                ("a2", false, 0),
                ("a1", false, 1)
            ]
        );
        // Opens on the step ⌘Z would undo.
        assert_eq!(State::default().current(&r), 2);
    }

    #[test]
    fn moving_stays_in_range() {
        let h = history(&["a"], &[]);
        let r = rows(&h);
        let mut s = State::default();
        s.move_by(&r, 5);
        assert_eq!(s.current(&r), 0);
        s.move_by(&r, -5);
        assert_eq!(s.current(&r), 0);
    }
}
