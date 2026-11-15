//! Working tree changes grouped like `git status`: conflicts, staged,
//! unstaged and untracked, with `git status -s` style codes.

use std::path::PathBuf;

use gf_core::model::{ChangeKind, ConflictKind, StatusEntry};
use iced::widget::{Column, row};
use iced::{Color, Element};

use crate::feature::{Cx, Feature, Update};
use crate::theme::Tokens;
use crate::widgets::{self, colored, dim, mono};

#[derive(Debug, Default)]
pub struct State {
    selected: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(PathBuf),
}

fn code(kind: ChangeKind) -> char {
    match kind {
        ChangeKind::Unmodified => ' ',
        ChangeKind::Modified => 'M',
        ChangeKind::TypeChanged => 'T',
        ChangeKind::Added => 'A',
        ChangeKind::Deleted => 'D',
        ChangeKind::Renamed => 'R',
        ChangeKind::Copied => 'C',
        ChangeKind::Unmerged => 'U',
    }
}

fn conflict_label(kind: ConflictKind) -> &'static str {
    match kind {
        ConflictKind::BothModified => "both modified",
        ConflictKind::BothAdded => "both added",
        ConflictKind::BothDeleted => "both deleted",
        ConflictKind::AddedByUs => "added by us",
        ConflictKind::AddedByThem => "added by them",
        ConflictKind::DeletedByUs => "deleted by us",
        ConflictKind::DeletedByThem => "deleted by them",
    }
}

#[derive(Default)]
struct Groups<'a> {
    conflicts: Vec<&'a StatusEntry>,
    staged: Vec<&'a StatusEntry>,
    unstaged: Vec<&'a StatusEntry>,
    untracked: Vec<&'a StatusEntry>,
}

fn group(entries: &[StatusEntry]) -> Groups<'_> {
    let mut g = Groups::default();
    for e in entries {
        match e {
            StatusEntry::Conflicted { .. } => g.conflicts.push(e),
            StatusEntry::Untracked { .. } => g.untracked.push(e),
            StatusEntry::Ignored { .. } => {}
            StatusEntry::Changed { .. } => {
                // A partly staged file appears in both groups, like `git status`.
                if e.is_staged() {
                    g.staged.push(e);
                }
                if e.has_unstaged_changes() {
                    g.unstaged.push(e);
                }
            }
        }
    }
    g
}

impl State {
    fn entry_row<'a>(
        &self,
        t: &'a Tokens,
        e: &'a StatusEntry,
        staged_view: bool,
    ) -> Element<'a, Message> {
        let (code_text, color, detail): (String, Color, Option<String>) = match e {
            StatusEntry::Changed {
                index,
                worktree,
                orig_path,
                ..
            } => {
                let (k, color) = if staged_view {
                    (*index, t.added())
                } else {
                    (*worktree, t.removed())
                };
                let detail = orig_path
                    .as_ref()
                    .filter(|_| staged_view)
                    .map(|p| format!("← {}", p.display()));
                (format!("{} ", code(k)), color, detail)
            }
            StatusEntry::Conflicted { kind, .. } => {
                ("U ".into(), t.error(), Some(conflict_label(*kind).into()))
            }
            StatusEntry::Untracked { .. } => ("? ".into(), t.dim, None),
            StatusEntry::Ignored { .. } => ("! ".into(), t.dim, None),
        };
        let path = e.path();
        let mut line = row![
            colored(t, code_text, color),
            mono(t, path.display().to_string())
        ]
        .spacing(4);
        if let Some(d) = detail {
            line = line.push(dim(t, d));
        }
        widgets::list_row(
            t,
            line,
            self.selected.as_ref() == Some(path),
            Message::Select(path.clone()),
        )
    }
}

impl Feature for State {
    type Message = Message;
    type Output = std::convert::Infallible;

    fn update(&mut self, message: Message) -> Update<Message, Self::Output> {
        match message {
            Message::Select(p) => self.selected = Some(p),
        }
        Update::none()
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let Some(snap) = cx.snapshot else {
            return widgets::panel(t, "CHANGES".into(), widgets::empty_hint(t, "loading…"));
        };
        let g = group(&snap.status.entries);
        let total = g.conflicts.len() + g.staged.len() + g.unstaged.len() + g.untracked.len();
        if total == 0 {
            return widgets::panel(
                t,
                "CHANGES".into(),
                widgets::empty_hint(t, "✓ working tree clean — edit files and they appear here"),
            );
        }

        let mut list = Column::new().spacing(1);
        let section = |list: Column<'a, Message>,
                       title: &str,
                       color: Color,
                       items: &[&'a StatusEntry],
                       staged: bool| {
            if items.is_empty() {
                return list;
            }
            let mut list =
                list.push(colored(t, format!("{title} ({})", items.len()), color).size(t.small()));
            for e in items {
                list = list.push(self.entry_row(t, e, staged));
            }
            list
        };
        list = section(list, "CONFLICTS", t.error(), &g.conflicts, false);
        list = section(list, "STAGED", t.added(), &g.staged, true);
        list = section(list, "UNSTAGED", t.removed(), &g.unstaged, false);
        list = section(list, "UNTRACKED", t.dim, &g.untracked, false);

        widgets::panel(t, format!("CHANGES ({total})"), widgets::scroll(list))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partly_staged_file_is_in_both_groups() {
        let entries = vec![
            StatusEntry::Changed {
                path: "a".into(),
                orig_path: None,
                index: ChangeKind::Modified,
                worktree: ChangeKind::Modified,
            },
            StatusEntry::Untracked { path: "b".into() },
            StatusEntry::Ignored { path: "c".into() },
        ];
        let g = group(&entries);
        assert_eq!(
            (g.staged.len(), g.unstaged.len(), g.untracked.len()),
            (1, 1, 1)
        );
    }
}
