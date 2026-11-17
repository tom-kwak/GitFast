//! Command log: every git command gf runs, with live output, exit status and
//! duration — the "what did that button actually do" panel.
//!
//! Background reads gf issues to refresh views are hidden by default (one
//! toggle shows them) so the log stays about the user's own actions.
//!
//! The list follows new output while it is scrolled to the bottom. Scrolling
//! up holds the position (new output no longer moves it); scrolling back to
//! the bottom follows again.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gf_core::event::CommandId;
use gf_core::{CommandEvent, CommandPhase};
use iced::widget::{Column, button, column, row, space};
use iced::{Element, Length, Padding, Task};

use crate::feature::{Cx, Feature, Update};
use crate::theme::{Tokens, style};
use crate::widgets::{self, colored, dim, format};

const MAX_ENTRIES: usize = 300;
const MAX_LINES_PER_ENTRY: usize = 2000;
/// Output longer than this starts collapsed.
const COLLAPSE_OVER: usize = 12;
/// The log's scrollable, for following new output.
const SCROLL_ID: &str = "gf-command-log";
/// Within this many pixels of the end counts as "at the bottom".
const BOTTOM_SLACK: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    Info,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
enum EntryStatus {
    Running {
        since: Instant,
    },
    Done {
        exit_code: Option<i32>,
        duration: Duration,
    },
    Note(NoteKind),
}

#[derive(Debug, Clone)]
struct Entry {
    command: Option<CommandId>,
    title: String,
    background: bool,
    lines: Vec<(String, bool)>,
    truncated: usize,
    status: EntryStatus,
    expanded: Option<bool>,
}

impl Entry {
    fn is_expanded(&self) -> bool {
        self.expanded.unwrap_or(self.lines.len() <= COLLAPSE_OVER)
    }
}

#[derive(Debug, Default)]
pub struct State {
    entries: VecDeque<Entry>,
    show_background: bool,
    /// Spinner frame for running commands, advanced by the app's tick.
    tick: usize,
    /// The user scrolled up to read something: new output must not move
    /// the list until they scroll back to the bottom.
    held: bool,
    /// Entries or lines were added since the last [`State::follow`].
    grew: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    ToggleEntry(usize),
    ToggleBackground,
    Clear,
    Scrolled(iced::widget::scrollable::Viewport),
}

/// Whether `v` shows the end of the list.
fn at_bottom(v: &iced::widget::scrollable::Viewport) -> bool {
    let rest = v.content_bounds().height - v.bounds().height - v.absolute_offset().y;
    rest <= BOTTOM_SLACK
}

impl State {
    pub fn on_command_event(&mut self, event: &CommandEvent) {
        match &event.phase {
            CommandPhase::Started { argv, read_only } => self.push(Entry {
                command: Some(event.id),
                title: argv.join(" "),
                background: *read_only,
                lines: Vec::new(),
                truncated: 0,
                status: EntryStatus::Running {
                    since: Instant::now(),
                },
                expanded: None,
            }),
            CommandPhase::Output { line, is_stderr } => {
                if let Some(e) = self.find(event.id) {
                    if e.lines.len() < MAX_LINES_PER_ENTRY {
                        e.lines.push((line.clone(), *is_stderr));
                    } else {
                        e.truncated += 1;
                    }
                    self.grew = true;
                }
            }
            CommandPhase::Finished {
                exit_code,
                duration,
            } => {
                if let Some(e) = self.find(event.id) {
                    e.status = EntryStatus::Done {
                        exit_code: *exit_code,
                        duration: *duration,
                    };
                }
            }
        }
    }

    /// Adds a message from gf itself (help text, parse errors, hints).
    pub fn note(&mut self, kind: NoteKind, title: impl Into<String>, lines: Vec<String>) {
        self.push(Entry {
            command: None,
            title: title.into(),
            background: false,
            lines: lines.into_iter().map(|l| (l, false)).collect(),
            truncated: 0,
            status: EntryStatus::Note(kind),
            expanded: Some(true),
        });
    }

    pub fn has_running(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e.status, EntryStatus::Running { .. }) && !e.background)
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn clear(&mut self) {
        self.entries
            .retain(|e| matches!(e.status, EntryStatus::Running { .. }));
        self.held = false;
    }

    /// Keeps the end of the list in view after it grew, unless the user is
    /// holding a position higher up. The app calls this after every update.
    pub fn follow(&mut self) -> Task<Message> {
        if std::mem::take(&mut self.grew) && !self.held {
            // A relative offset of 1.0 stays at the end as lines keep coming.
            iced::widget::operation::snap_to_end(SCROLL_ID)
        } else {
            Task::none()
        }
    }

    fn push(&mut self, entry: Entry) {
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
        self.grew = true;
    }

    fn find(&mut self, id: CommandId) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .rev()
            .find(|e| e.command == Some(id))
    }

    fn entry_view<'a>(&'a self, t: &'a Tokens, index: usize, e: &'a Entry) -> Element<'a, Message> {
        let (marker, marker_color) = match &e.status {
            EntryStatus::Note(NoteKind::Info) => ("»", t.accent()),
            EntryStatus::Note(NoteKind::Error) => ("!", t.error()),
            _ => ("$", if e.background { t.dim } else { t.success() }),
        };
        let status: Element<'a, Message> = match &e.status {
            EntryStatus::Running { since } => colored(
                t,
                format!(
                    "{} {}",
                    widgets::spinner_frame(self.tick),
                    format::duration(since.elapsed())
                ),
                t.warning(),
            )
            .into(),
            EntryStatus::Done {
                exit_code: Some(0),
                duration,
            } => colored(t, format!("✓ {}", format::duration(*duration)), t.success()).into(),
            EntryStatus::Done {
                exit_code,
                duration,
            } => colored(
                t,
                format!(
                    "✗ {} · {}",
                    exit_code.map_or("killed".to_owned(), |c| format!("exit {c}")),
                    format::duration(*duration)
                ),
                t.error(),
            )
            .into(),
            EntryStatus::Note(_) => space().into(),
        };

        let failed = matches!(e.status, EntryStatus::Done { exit_code, .. } if exit_code != Some(0))
            || e.status == EntryStatus::Note(NoteKind::Error);
        let expanded = e.is_expanded();
        let fold = if e.lines.is_empty() {
            " "
        } else if expanded {
            "▾"
        } else {
            "▸"
        };
        let title_color = if e.background { t.dim } else { t.fg };
        let header = row![
            dim(t, fold),
            colored(t, marker, marker_color),
            colored(t, &e.title, title_color).width(Length::Fill),
            status,
        ]
        .spacing(6);

        // Clicking the header folds/unfolds long output.
        let header = button(header)
            .width(Length::Fill)
            .padding(Padding::from([1, 4]))
            .style(style::row_button(t, false))
            .on_press(Message::ToggleEntry(index));

        let mut col = Column::new().push(header);
        if expanded {
            for (line, is_stderr) in &e.lines {
                let color = match (*is_stderr, failed) {
                    (true, true) => t.error(),
                    (true, false) => t.dim,
                    (false, _) => t.fg,
                };
                col = col.push(colored(t, format!("    {line}"), color));
            }
            if e.truncated > 0 {
                col = col.push(dim(t, format!("    … {} more lines not kept", e.truncated)));
            }
        } else if !e.lines.is_empty() {
            col = col.push(dim(
                t,
                format!("    {} lines — click to expand", e.lines.len()),
            ));
        }
        col.into()
    }
}

impl Feature for State {
    type Message = Message;
    type Output = std::convert::Infallible;

    fn update(&mut self, message: Message) -> Update<Message, Self::Output> {
        match message {
            Message::ToggleEntry(i) => {
                if let Some(e) = self.entries.get_mut(i) {
                    e.expanded = Some(!e.is_expanded());
                }
            }
            Message::ToggleBackground => self.show_background = !self.show_background,
            Message::Clear => self.clear(),
            Message::Scrolled(v) => {
                self.held = !at_bottom(&v);
                if !self.held {
                    // Back at the bottom (by wheel or scrollbar): pin it there
                    // so the next output is followed.
                    return Update::task(iced::widget::operation::snap_to_end(SCROLL_ID));
                }
            }
        }
        Update::none()
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let hidden = self.entries.iter().filter(|e| e.background).count();
        let toolbar = row![
            dim(t, "─ COMMAND LOG ").width(Length::Fill),
            widgets::text_button(
                t,
                if self.show_background {
                    "hide background reads"
                } else {
                    "show background reads"
                },
                None,
                Some(Message::ToggleBackground),
            ),
            widgets::text_button(t, "clear", None, Some(Message::Clear)),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);

        let visible: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.show_background || !e.background)
            .collect();

        let body: Element<'a, Message> = if visible.is_empty() {
            let hint = if hidden > 0 && !self.show_background {
                format!(
                    "no commands yet ({hidden} background reads hidden) — type a git command below"
                )
            } else {
                "no commands yet — type a git command below, e.g. status".into()
            };
            widgets::empty_hint(t, hint)
        } else {
            let list = visible
                .into_iter()
                .fold(Column::new().spacing(2), |col, (i, e)| {
                    col.push(self.entry_view(t, i, e))
                });
            iced::widget::scrollable(list)
                .id(SCROLL_ID)
                .on_scroll(Message::Scrolled)
                .direction(iced::widget::scrollable::Direction::Vertical(
                    iced::widget::scrollable::Scrollbar::new()
                        .width(6)
                        .scroller_width(6),
                ))
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };

        iced::widget::container(column![toolbar, body].spacing(2))
            .padding(Padding::from([2, 6]))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(style::panel(t))
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: CommandId, phase: CommandPhase) -> CommandEvent {
        CommandEvent {
            id,
            repo: None,
            phase,
        }
    }

    #[test]
    fn tracks_command_lifecycle() {
        let mut s = State::default();
        s.on_command_event(&ev(
            1,
            CommandPhase::Started {
                argv: vec!["git".into(), "push".into()],
                read_only: false,
            },
        ));
        assert!(s.has_running());
        s.on_command_event(&ev(
            1,
            CommandPhase::Output {
                line: "done".into(),
                is_stderr: true,
            },
        ));
        s.on_command_event(&ev(
            1,
            CommandPhase::Finished {
                exit_code: Some(0),
                duration: Duration::from_millis(5),
            },
        ));
        assert!(!s.has_running());
        let e = &s.entries[0];
        assert_eq!(e.title, "git push");
        assert_eq!(e.lines, vec![("done".to_owned(), true)]);
    }

    #[test]
    fn long_output_starts_collapsed_and_toggles() {
        let mut s = State::default();
        s.on_command_event(&ev(
            7,
            CommandPhase::Started {
                argv: vec!["git".into(), "log".into()],
                read_only: false,
            },
        ));
        for i in 0..20 {
            s.on_command_event(&ev(
                7,
                CommandPhase::Output {
                    line: i.to_string(),
                    is_stderr: false,
                },
            ));
        }
        assert!(!s.entries[0].is_expanded());
        let _ = s.update(Message::ToggleEntry(0));
        assert!(s.entries[0].is_expanded());
    }

    #[test]
    fn new_output_is_followed_unless_the_user_holds_a_position() {
        let mut s = State::default();
        assert_eq!(s.follow().units(), 0, "nothing new");
        s.note(NoteKind::Info, "a", vec![]);
        assert!(s.follow().units() > 0, "at the bottom: follow");
        assert_eq!(s.follow().units(), 0, "only once per change");

        s.held = true;
        s.note(NoteKind::Info, "b", vec![]);
        assert_eq!(s.follow().units(), 0, "scrolled up: stay put");

        s.clear();
        s.note(NoteKind::Info, "c", vec![]);
        assert!(s.follow().units() > 0, "clearing follows again");
    }

    #[test]
    fn clear_keeps_running_commands_and_caps_entries() {
        let mut s = State::default();
        for i in 0..(MAX_ENTRIES as u64 + 5) {
            s.on_command_event(&ev(
                i,
                CommandPhase::Started {
                    argv: vec!["git".into()],
                    read_only: true,
                },
            ));
        }
        assert_eq!(s.entries.len(), MAX_ENTRIES);
        s.note(NoteKind::Info, "hello", vec![]);
        s.clear();
        assert!(
            s.entries
                .iter()
                .all(|e| matches!(e.status, EntryStatus::Running { .. }))
        );
    }
}
