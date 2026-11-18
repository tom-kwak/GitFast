//! Interactive rebase editor: the commits after a base, oldest first, with
//! an action per commit (pick / reword / squash / fixup / drop), ↑/↓ to
//! reorder, and inline message editing — instead of a todo file in an
//! editor. Starting it goes through the shared change confirmation.

use std::collections::HashMap;

use gf_core::model::Oid;
use gf_git::{TodoAction, TodoItem};
use iced::widget::text::Wrapping;
use iced::widget::{Column, button, column, container, row, space, text_editor};
use iced::{Background, Border, Element, Length, Padding};

use crate::theme::{Tokens, style};
use crate::widgets::{self, colored, dim, mono};

pub struct State {
    pub base: Oid,
    pub items: Vec<TodoItem>,
    /// Commit ids in their original order, to tell whether rows were moved.
    original: Vec<Oid>,
    error: Option<String>,
    /// Multi-line message editors of the reword/squash rows, by commit;
    /// their text is kept in the row's action (see `sync_editors`).
    editors: HashMap<Oid, text_editor::Content>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("base", &self.base)
            .field("items", &self.items)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Next action for row `i` (pick → reword → squash → fixup → drop).
    Cycle(usize),
    MoveUp(usize),
    MoveDown(usize),
    /// Edit in the message editor of row `i`.
    Edit(usize, text_editor::Action),
    Start,
    Cancel,
}

#[derive(Debug, Clone)]
pub enum Output {
    Start { base: Oid, items: Vec<TodoItem> },
    Cancel,
}

fn full_message(item: &TodoItem) -> String {
    if item.commit.body.is_empty() {
        item.commit.summary.clone()
    } else {
        format!("{}\n\n{}", item.commit.summary, item.commit.body)
    }
}

fn action_label(a: &TodoAction) -> (&'static str, &'static str) {
    match a {
        TodoAction::Pick => ("pick  ", "keep as is"),
        TodoAction::Reword(_) => ("reword", "keep, with a new message"),
        TodoAction::Squash(_) => (
            "squash",
            "meld into the commit above, edit the combined message",
        ),
        TodoAction::Fixup => ("fixup ", "meld into the commit above, keep its message"),
        TodoAction::Drop => ("drop  ", "remove this commit"),
    }
}

impl State {
    pub fn new(base: Oid, items: Vec<TodoItem>) -> Self {
        let mut state = Self {
            base,
            original: items.iter().map(|i| i.commit.id.clone()).collect(),
            items,
            error: None,
            editors: HashMap::new(),
        };
        state.sync_editors();
        state
    }

    /// An editor for every row with a message (filled from the action),
    /// none for the others.
    fn sync_editors(&mut self) {
        let mut keep = HashMap::new();
        for item in &self.items {
            if let TodoAction::Reword(m) | TodoAction::Squash(m) = &item.action {
                let id = &item.commit.id;
                let editor = self
                    .editors
                    .remove(id)
                    .unwrap_or_else(|| text_editor::Content::with_text(m));
                keep.insert(id.clone(), editor);
            }
        }
        self.editors = keep;
    }

    /// Presets the plan to meld `ids` (list order, newest first) into one
    /// commit: the oldest is picked, the rest folded in with the combined
    /// message. They must be adjacent in the plan.
    pub fn squash(mut self, ids: &[Oid]) -> Result<Self, String> {
        let range = gf_git::squash_range(&self.items, ids).map_err(|e| e.to_string())?;
        let message = self.items[range.clone()]
            .iter()
            .map(full_message)
            .collect::<Vec<_>>()
            .join("\n\n");
        self.items = gf_git::squash_plan(std::mem::take(&mut self.items), range, message);
        self.sync_editors();
        Ok(self)
    }

    fn next_action(&self, i: usize) -> TodoAction {
        let item = &self.items[i];
        let above = i.checked_sub(1).and_then(|j| self.items.get(j));
        match item.action {
            TodoAction::Pick => TodoAction::Reword(full_message(item)),
            // The first commit has nothing above to meld into.
            TodoAction::Reword(_) if i == 0 => TodoAction::Drop,
            TodoAction::Reword(_) => TodoAction::Squash(format!(
                "{}\n\n{}",
                above.map(full_message).unwrap_or_default(),
                full_message(item)
            )),
            TodoAction::Squash(_) => TodoAction::Fixup,
            TodoAction::Fixup => TodoAction::Drop,
            TodoAction::Drop => TodoAction::Pick,
        }
    }

    /// Short summary of what will happen.
    fn summary(&self) -> String {
        let count = |f: fn(&TodoAction) -> bool| self.items.iter().filter(|i| f(&i.action)).count();
        let reorder = self.reordered();
        let mut parts = vec![format!("{} commits", self.items.len())];
        for (n, word) in [
            (count(|a| matches!(a, TodoAction::Reword(_))), "reworded"),
            (
                count(|a| matches!(a, TodoAction::Squash(_) | TodoAction::Fixup)),
                "melded",
            ),
            (count(|a| matches!(a, TodoAction::Drop)), "dropped"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {word}"));
            }
        }
        if reorder {
            parts.push("reordered".into());
        }
        parts.join(" · ")
    }

    /// Whether rows were moved. Compared by id: author dates are not
    /// reliable (equal or out-of-order dates are common after a rebase).
    fn reordered(&self) -> bool {
        !self
            .items
            .iter()
            .map(|i| &i.commit.id)
            .eq(self.original.iter())
    }

    fn is_noop(&self) -> bool {
        self.items.iter().all(|i| i.action == TodoAction::Pick) && !self.reordered()
    }

    pub fn update(&mut self, message: Message) -> Option<Output> {
        self.error = None;
        match message {
            Message::Cycle(i) if i < self.items.len() => {
                self.items[i].action = self.next_action(i);
                // A new action comes with a new prefilled message.
                self.editors.remove(&self.items[i].commit.id);
                self.sync_editors();
                None
            }
            Message::MoveUp(i) if i > 0 && i < self.items.len() => {
                self.items.swap(i, i - 1);
                None
            }
            Message::MoveDown(i) if i + 1 < self.items.len() => {
                self.items.swap(i, i + 1);
                None
            }
            Message::Edit(i, action) => {
                let item = self.items.get_mut(i)?;
                if let (Some(editor), TodoAction::Reword(m) | TodoAction::Squash(m)) =
                    (self.editors.get_mut(&item.commit.id), &mut item.action)
                {
                    editor.perform(action);
                    *m = editor.text();
                }
                None
            }
            Message::Start => {
                if self.is_noop() {
                    self.error = Some("nothing to change — pick an action or reorder first".into());
                    return None;
                }
                if gf_git::melds_into_nothing(&self.items) {
                    self.error = Some(
                        "the first kept commit cannot be squashed or fixed up: nothing above it to meld into"
                            .into(),
                    );
                    return None;
                }
                Some(Output::Start {
                    base: self.base.clone(),
                    items: self.items.clone(),
                })
            }
            Message::Cancel => Some(Output::Cancel),
            _ => None,
        }
    }

    pub fn view<'a>(&'a self, t: &'a Tokens) -> Element<'a, Message> {
        let accent = t.warning();
        let mut list = Column::new().spacing(2);
        for (i, item) in self.items.iter().enumerate() {
            let (label, hint) = action_label(&item.action);
            let color = match item.action {
                TodoAction::Drop => t.error(),
                TodoAction::Pick => t.dim,
                _ => t.accent(),
            };
            let action = widgets::tip(
                t,
                button(colored(t, format!("[{label} ▸]"), color))
                    .padding(Padding::from([1, 2]))
                    .style(style::text_button(t, color))
                    .on_press(Message::Cycle(i)),
                format!("{hint} — click to change"),
            );
            let summary_color = if item.action == TodoAction::Drop {
                t.dim
            } else {
                t.fg
            };
            let mut line = row![
                action,
                colored(t, item.commit.id.short(), t.warning()),
                colored(t, item.commit.summary.clone(), summary_color)
                    .wrapping(Wrapping::None)
                    .width(Length::Fill),
                // Nothing above the first row or below the last.
                widgets::icon_button_maybe(
                    t,
                    "↑",
                    t.accent(),
                    "move up (earlier)",
                    (i > 0).then_some(Message::MoveUp(i)),
                ),
                widgets::icon_button_maybe(
                    t,
                    "↓",
                    t.accent(),
                    "move down (later)",
                    (i + 1 < self.items.len()).then_some(Message::MoveDown(i)),
                ),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center);
            if item.action == TodoAction::Drop {
                line = line.push(dim(t, "removed"));
            }
            let mut cell = column![line];
            if let Some(editor) = self.editors.get(&item.commit.id) {
                let lines = editor.line_count().clamp(2, 8);
                #[allow(clippy::cast_precision_loss)]
                let height = (lines as f32 + 0.6) * t.font_size * 1.3;
                cell = cell.push(
                    row![
                        dim(t, "      message"),
                        text_editor(editor)
                            .placeholder("commit message — first line is the summary")
                            .on_action(move |a| Message::Edit(i, a))
                            .size(t.font_size)
                            .height(Length::Fixed(height))
                            .style(style::text_area(t)),
                    ]
                    .spacing(6),
                );
            }
            list = list.push(cell);
        }

        let mut col = column![
            colored(
                t,
                format!("┌─ ? Interactive rebase onto {}", self.base.short()),
                accent
            ),
            mono(
                t,
                "│ oldest first · click an action to change it · ↑/↓ to reorder"
            ),
            container(widgets::scroll(list)).max_height(t.font_size * 30.0),
            dim(t, format!("│ {}", self.summary())),
        ]
        .spacing(6);
        if let Some(e) = &self.error {
            col = col.push(colored(t, format!("│ ⚠ {e}"), t.error()));
        }
        col = col.push(
            row![
                space().width(Length::Fill),
                widgets::text_button(t, "Cancel", Some("Esc"), Some(Message::Cancel)),
                widgets::text_button(t, "Start rebase", None, Some(Message::Start)),
            ]
            .spacing(6),
        );

        container(col)
            .padding(Padding::from([10, 14]))
            .max_width(900)
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
    use gf_core::model::{Commit, Signature, Timestamp};

    fn item(n: u8, t: i64) -> TodoItem {
        let sig = Signature {
            name: String::new(),
            email: String::new(),
            time: Timestamp {
                seconds: t,
                offset_minutes: 0,
            },
        };
        TodoItem {
            action: TodoAction::Pick,
            commit: Commit {
                id: format!("{n:040x}").parse().expect("oid"),
                parents: vec![],
                author: sig.clone(),
                committer: sig,
                summary: format!("c{n}"),
                body: String::new(),
            },
        }
    }

    fn editor() -> State {
        State::new(
            "f".repeat(40).parse().expect("oid"),
            vec![item(1, 1), item(2, 2), item(3, 3)],
        )
    }

    #[test]
    fn squash_preset_needs_adjacent_commits() {
        let id = |n: u8| -> Oid { format!("{n:040x}").parse().expect("oid") };
        let e = editor().squash(&[id(3), id(2)]).expect("adjacent");
        assert_eq!(e.items[0].action, TodoAction::Pick);
        assert_eq!(e.items[1].action, TodoAction::Pick);
        assert_eq!(e.items[2].action, TodoAction::Squash("c2\n\nc3".into()));
        let e = editor().squash(&[id(1), id(2), id(3)]).expect("all");
        assert_eq!(e.items[1].action, TodoAction::Fixup);
        assert_eq!(
            e.items[2].action,
            TodoAction::Squash("c1\n\nc2\n\nc3".into())
        );
        assert!(editor().squash(&[id(1), id(3)]).is_err());
        assert!(editor().squash(&[id(9), id(1)]).is_err());
    }

    #[test]
    fn messages_are_edited_over_several_lines() {
        let mut e = editor();
        let _ = e.update(Message::Cycle(1));
        let _ = e.update(Message::Edit(
            1,
            text_editor::Action::Move(text_editor::Motion::DocumentEnd),
        ));
        for c in ['\n', '\n', 'x'] {
            let _ = e.update(Message::Edit(
                1,
                text_editor::Action::Edit(text_editor::Edit::Insert(c)),
            ));
        }
        assert_eq!(e.items[1].action, TodoAction::Reword("c2\n\nx".into()));
        // Changing the action starts from a fresh prefilled message.
        let _ = e.update(Message::Cycle(1));
        assert_eq!(e.items[1].action, TodoAction::Squash("c1\n\nc2".into()));
        assert_eq!(
            e.editors
                .get(&e.items[1].commit.id)
                .map(text_editor::Content::text),
            Some("c1\n\nc2".to_owned())
        );
        let _ = e.update(Message::Cycle(1));
        assert!(e.editors.is_empty(), "fixup has no message");
    }

    #[test]
    fn cycles_actions_and_prefills_messages() {
        let mut e = editor();
        let _ = e.update(Message::Cycle(1));
        assert_eq!(e.items[1].action, TodoAction::Reword("c2".into()));
        let _ = e.update(Message::Cycle(1));
        assert_eq!(e.items[1].action, TodoAction::Squash("c1\n\nc2".into()));
        let _ = e.update(Message::Cycle(1));
        assert_eq!(e.items[1].action, TodoAction::Fixup);
        // The first row never offers squash/fixup.
        let _ = e.update(Message::Cycle(0));
        let _ = e.update(Message::Cycle(0));
        assert_eq!(e.items[0].action, TodoAction::Drop);
    }

    #[test]
    fn reordering_commits_with_equal_dates_counts_as_a_change() {
        let mut e = State::new(
            "f".repeat(40).parse().expect("oid"),
            vec![item(1, 5), item(2, 5)],
        );
        let _ = e.update(Message::MoveUp(1));
        assert!(e.summary().contains("reordered"));
        assert!(matches!(
            e.update(Message::Start),
            Some(Output::Start { .. })
        ));
        // Moving it back is a no-op again.
        let mut e = editor();
        let _ = e.update(Message::MoveUp(1));
        let _ = e.update(Message::MoveDown(0));
        assert!(e.update(Message::Start).is_none());
    }

    #[test]
    fn a_dropped_first_commit_does_not_let_the_next_one_meld() {
        let mut e = editor();
        e.items[0].action = TodoAction::Drop;
        e.items[1].action = TodoAction::Fixup;
        assert!(e.update(Message::Start).is_none());
        assert!(e.error.is_some());
    }

    #[test]
    fn start_requires_a_change_and_reports_plan() {
        let mut e = editor();
        assert!(e.update(Message::Start).is_none(), "no-op plan refused");
        assert!(e.error.is_some());
        let _ = e.update(Message::MoveUp(2));
        assert_eq!(e.items[1].commit.summary, "c3");
        assert!(e.summary().contains("reordered"));
        assert!(
            matches!(e.update(Message::Start), Some(Output::Start { items, .. }) if items.len() == 3)
        );
    }
}
