//! Working tree changes grouped like `git status` (conflicts, staged,
//! unstaged, untracked) with per-row and per-group actions. Clicking a row
//! shows its diff.

use std::path::PathBuf;

use gf_core::model::{ChangeKind, ConflictKind, StatusEntry};
use iced::widget::text::Wrapping;
use iced::widget::{container, row};
use iced::{Color, Element, Length};

use crate::feature::{Cx, Feature, Update};
use crate::features::diff_view::DiffSource;
use crate::theme::Tokens;
use crate::widgets::virtual_list::{ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim, mono};

/// Which group a row is listed in (a partly staged file is in two).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Conflicts,
    Staged,
    Unstaged,
    Untracked,
}

#[derive(Debug, Default)]
pub struct State {
    selected: Option<(PathBuf, Group)>,
    scroll: ScrollPos,
}

/// One row of the flattened list: a group header or a file.
#[derive(Debug, Clone, Copy)]
enum WtRow<'a> {
    Header {
        title: &'static str,
        color: Color,
        count: usize,
        action: Option<HeaderAction>,
    },
    Entry(&'a StatusEntry, Group),
}

/// Header buttons (kept `Copy` for the row enum).
#[derive(Debug, Clone, Copy)]
enum HeaderAction {
    StageAll,
    UnstageAll,
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(PathBuf, Group),
    Stage(PathBuf),
    Unstage(PathBuf),
    Discard(PathBuf, Group),
    StageAll,
    UnstageAll,
    Scrolled(ScrollPos),
}

#[derive(Debug, Clone)]
pub enum Output {
    ShowDiff(DiffSource),
    Stage(Vec<PathBuf>),
    Unstage(Vec<PathBuf>),
    /// Needs confirmation by the app before running.
    Discard {
        tracked: Vec<PathBuf>,
        untracked: Vec<PathBuf>,
    },
    StageAll,
    UnstageAll,
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
pub struct Groups<'a> {
    pub conflicts: Vec<&'a StatusEntry>,
    pub staged: Vec<&'a StatusEntry>,
    pub unstaged: Vec<&'a StatusEntry>,
    pub untracked: Vec<&'a StatusEntry>,
}

pub fn group(entries: &[StatusEntry]) -> Groups<'_> {
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

fn diff_source(path: PathBuf, group: Group) -> DiffSource {
    match group {
        Group::Staged => DiffSource::Staged(path),
        Group::Untracked => DiffSource::Untracked(path),
        Group::Conflicts | Group::Unstaged => DiffSource::WorkTree(path),
    }
}

impl State {
    /// Keeps the selection pointing at something that still exists, moving
    /// it between groups when a file was staged/unstaged. Returns the diff
    /// to show if the selection moved.
    pub fn follow(&mut self, entries: &[StatusEntry]) -> Option<DiffSource> {
        let (path, group) = self.selected.clone()?;
        let g = self::group(entries);
        let has = |list: &Vec<&StatusEntry>| list.iter().any(|e| e.path() == &path);
        let in_group = match group {
            Group::Conflicts => has(&g.conflicts),
            Group::Staged => has(&g.staged),
            Group::Unstaged => has(&g.unstaged),
            Group::Untracked => has(&g.untracked),
        };
        if in_group {
            return None;
        }
        let moved = [
            (Group::Staged, has(&g.staged)),
            (Group::Unstaged, has(&g.unstaged)),
            (Group::Untracked, has(&g.untracked)),
            (Group::Conflicts, has(&g.conflicts)),
        ]
        .into_iter()
        .find(|(_, present)| *present)
        .map(|(g, _)| g);
        match moved {
            Some(group) => {
                self.selected = Some((path.clone(), group));
                Some(diff_source(path, group))
            }
            None => {
                self.selected = None;
                None
            }
        }
    }

    fn entry_row<'a>(
        &self,
        t: &'a Tokens,
        e: &'a StatusEntry,
        group: Group,
    ) -> Element<'a, Message> {
        let (code_text, color, detail): (String, Color, Option<String>) = match e {
            StatusEntry::Changed {
                index,
                worktree,
                orig_path,
                ..
            } => {
                let staged = group == Group::Staged;
                let (k, color) = if staged {
                    (*index, t.added())
                } else {
                    (*worktree, t.removed())
                };
                let detail = orig_path
                    .as_ref()
                    .filter(|_| staged)
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
            mono(t, path.display().to_string()).wrapping(Wrapping::None)
        ]
        .spacing(4);
        if let Some(d) = detail {
            line = line.push(dim(t, d).wrapping(Wrapping::None));
        }
        let selected = self
            .selected
            .as_ref()
            .is_some_and(|(p, g)| p == path && *g == group);
        let main = widgets::list_row(t, line, selected, Message::Select(path.clone(), group));

        let actions: Element<'a, Message> = match group {
            Group::Staged => widgets::icon_button(
                t,
                "−",
                t.removed(),
                "unstage",
                Message::Unstage(path.clone()),
            ),
            Group::Unstaged => row![
                widgets::icon_button(
                    t,
                    "↺",
                    t.dim,
                    "discard changes…",
                    Message::Discard(path.clone(), group)
                ),
                widgets::icon_button(t, "+", t.added(), "stage", Message::Stage(path.clone())),
            ]
            .into(),
            Group::Untracked => row![
                widgets::icon_button(
                    t,
                    "✕",
                    t.dim,
                    "delete file…",
                    Message::Discard(path.clone(), group)
                ),
                widgets::icon_button(
                    t,
                    "+",
                    t.added(),
                    "stage (start tracking)",
                    Message::Stage(path.clone())
                ),
            ]
            .into(),
            Group::Conflicts => widgets::icon_button(
                t,
                "✓",
                t.added(),
                "mark resolved (stage)",
                Message::Stage(path.clone()),
            ),
        };
        // Actions on the left so they stay visible when long paths scroll.
        row![actions, main].align_y(iced::Alignment::Center).into()
    }
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message {
            Message::Select(path, group) => {
                self.selected = Some((path.clone(), group));
                Update::output(Output::ShowDiff(diff_source(path, group)))
            }
            Message::Stage(p) => Update::output(Output::Stage(vec![p])),
            Message::Unstage(p) => Update::output(Output::Unstage(vec![p])),
            Message::Discard(p, Group::Untracked) => Update::output(Output::Discard {
                tracked: Vec::new(),
                untracked: vec![p],
            }),
            Message::Discard(p, _) => Update::output(Output::Discard {
                tracked: vec![p],
                untracked: Vec::new(),
            }),
            Message::StageAll => Update::output(Output::StageAll),
            Message::UnstageAll => Update::output(Output::UnstageAll),
            Message::Scrolled(pos) => {
                self.scroll = pos;
                Update::none()
            }
        }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let Some(snap) = cx.snapshot else {
            return widgets::panel(t, "WORKING TREE".into(), widgets::empty_hint(t, "loading…"));
        };
        let g = group(&snap.status.entries);
        let total = g.conflicts.len() + g.staged.len() + g.unstaged.len() + g.untracked.len();
        if total == 0 {
            return widgets::panel(
                t,
                "WORKING TREE".into(),
                widgets::empty_hint(t, "✓ clean — edit files and they appear here"),
            );
        }

        let mut rows: Vec<WtRow<'a>> = Vec::new();
        let mut section = |title: &'static str,
                           color: Color,
                           items: &[&'a StatusEntry],
                           group: Group,
                           action: Option<HeaderAction>| {
            if items.is_empty() {
                return;
            }
            rows.push(WtRow::Header {
                title,
                color,
                count: items.len(),
                action,
            });
            rows.extend(items.iter().map(|e| WtRow::Entry(e, group)));
        };
        section("CONFLICTS", t.error(), &g.conflicts, Group::Conflicts, None);
        section(
            "STAGED",
            t.added(),
            &g.staged,
            Group::Staged,
            Some(HeaderAction::UnstageAll),
        );
        let has_unstaged = !g.unstaged.is_empty() || !g.untracked.is_empty();
        section(
            "UNSTAGED",
            t.removed(),
            &g.unstaged,
            Group::Unstaged,
            has_unstaged.then_some(HeaderAction::StageAll),
        );
        section("UNTRACKED", t.dim, &g.untracked, Group::Untracked, None);

        let path_cols = snap
            .status
            .entries
            .iter()
            .map(|e| {
                let orig = match e {
                    StatusEntry::Changed {
                        orig_path: Some(o), ..
                    } => columns(&o.display().to_string()) + 3,
                    StatusEntry::Conflicted { .. } => 16,
                    _ => 0,
                };
                columns(&e.path().display().to_string()) + orig
            })
            .max()
            .unwrap_or(0);
        // two action icons, status code, path (+ rename source) and padding.
        let content_width = 64.0 + t.cols(3 + path_cols) + 24.0;
        let list = virtual_list(
            "gf-working-tree",
            rows.len(),
            (t.font_size * 1.8).round(),
            content_width,
            self.scroll,
            move |i| match rows[i] {
                WtRow::Header {
                    title,
                    color,
                    count,
                    action,
                } => {
                    let mut header =
                        row![colored(t, format!("{title} ({count})"), color).size(t.small())]
                            .spacing(8)
                            .align_y(iced::Alignment::Center);
                    if let Some(a) = action {
                        let (label, msg) = match a {
                            HeaderAction::StageAll => ("stage all", Message::StageAll),
                            HeaderAction::UnstageAll => ("unstage all", Message::UnstageAll),
                        };
                        header = header.push(widgets::text_button(t, label, None, Some(msg)));
                    }
                    container(header)
                        .height(Length::Fill)
                        .align_y(iced::Alignment::Center)
                        .into()
                }
                WtRow::Entry(e, group) => self.entry_row(t, e, group),
            },
            Message::Scrolled,
        );
        widgets::panel(t, format!("WORKING TREE ({total})"), list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(path: &str, index: ChangeKind, worktree: ChangeKind) -> StatusEntry {
        StatusEntry::Changed {
            path: path.into(),
            orig_path: None,
            index,
            worktree,
        }
    }

    #[test]
    fn partly_staged_file_is_in_both_groups() {
        let entries = vec![
            changed("a", ChangeKind::Modified, ChangeKind::Modified),
            StatusEntry::Untracked { path: "b".into() },
            StatusEntry::Ignored { path: "c".into() },
        ];
        let g = group(&entries);
        assert_eq!(
            (g.staged.len(), g.unstaged.len(), g.untracked.len()),
            (1, 1, 1)
        );
    }

    #[test]
    fn selection_follows_a_file_that_got_staged() {
        let mut s = State::default();
        let _ = s.update(Message::Select("a".into(), Group::Unstaged));
        let after = vec![changed("a", ChangeKind::Modified, ChangeKind::Unmodified)];
        assert_eq!(s.follow(&after), Some(DiffSource::Staged("a".into())));
        assert_eq!(s.follow(&after), None, "already in the right group");
        assert_eq!(s.follow(&[]), None);
        assert!(
            s.selected.is_none(),
            "selection cleared when the file is clean"
        );
    }

    #[test]
    fn discard_of_untracked_deletes_the_file() {
        let mut s = State::default();
        let out = s
            .update(Message::Discard("u".into(), Group::Untracked))
            .output;
        assert!(
            matches!(out, Some(Output::Discard { tracked, untracked }) if tracked.is_empty() && untracked.len() == 1)
        );
    }
}
