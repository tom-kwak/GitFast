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
use crate::nav::{self, Key, Moved, Nav};
use crate::repo_tab::LineStats;
use crate::theme::Tokens;
use crate::widgets::virtual_list::{ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim};

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
    /// A conflicted file was picked: open the conflict resolver.
    ResolveConflict(PathBuf),
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

/// Line counts for an entry as listed in `group`.
fn entry_counts(lines: &LineStats, path: &std::path::Path, group: Group) -> widgets::EntryCounts {
    let map = match group {
        Group::Staged => &lines.staged,
        Group::Unstaged | Group::Conflicts => &lines.unstaged,
        Group::Untracked => &lines.untracked,
    };
    map.get(path).copied()
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

/// Rows as listed: `None` for a group header, then its files.
fn flat(entries: &[StatusEntry]) -> Vec<Option<(PathBuf, Group)>> {
    let g = group(entries);
    let mut rows = Vec::new();
    for (items, group) in [
        (&g.conflicts, Group::Conflicts),
        (&g.staged, Group::Staged),
        (&g.unstaged, Group::Unstaged),
        (&g.untracked, Group::Untracked),
    ] {
        if items.is_empty() {
            continue;
        }
        rows.push(None);
        rows.extend(items.iter().map(|e| Some((e.path().clone(), group))));
    }
    rows
}

fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.8).round()
}

/// Scrollable id of the list.
const LIST_ID: &str = "gf-working-tree";

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

    /// Keys for the list: arrows pick a file, Space stages or unstages
    /// it, `d` discards it (the app confirms first).
    pub fn nav(&self, nav: Nav, entries: &[StatusEntry], t: &Tokens) -> Option<Moved<Message>> {
        let rows = flat(entries);
        let files: Vec<(usize, &(PathBuf, Group))> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|f| (i, f)))
            .collect();
        let current = self
            .selected
            .as_ref()
            .and_then(|sel| files.iter().position(|(_, f)| *f == sel));
        let selected = current.map(|c| files[c].1.clone());
        match nav.key {
            Key::Space => {
                let (path, group) = selected?;
                return Some(Moved::message(match group {
                    Group::Staged => Message::Unstage(path),
                    Group::Unstaged | Group::Untracked => Message::Stage(path),
                    Group::Conflicts => return Some(Moved::stay()),
                }));
            }
            Key::Char('d') => {
                let (path, group) = selected?;
                return Some(match group {
                    Group::Unstaged | Group::Untracked => {
                        Moved::message(Message::Discard(path, group))
                    }
                    Group::Staged | Group::Conflicts => Moved::stay(),
                });
            }
            _ => {}
        }
        let h = row_height(t);
        let i = nav::step(current, files.len(), nav.key, nav::page(self.scroll, h))?;
        if current == Some(i) {
            return Some(Moved::stay());
        }
        let (row, (path, group)) = files[i];
        Some(
            Moved::message(Message::Select(path.clone(), *group)).with_task(nav::reveal(
                LIST_ID,
                self.scroll,
                row,
                h,
            )),
        )
    }

    fn entry_row<'a>(
        &self,
        t: &'a Tokens,
        e: &'a StatusEntry,
        group: Group,
        lines: &LineStats,
    ) -> Element<'a, Message> {
        let (code_text, color, detail): (String, Color, Option<String>) = match e {
            StatusEntry::Changed {
                index,
                worktree,
                orig_path,
                ..
            } => {
                let staged = group == Group::Staged;
                let k = if staged { *index } else { *worktree };
                let color = widgets::change_color(t, code(k));
                let detail = orig_path
                    .as_ref()
                    .filter(|_| staged)
                    .map(|p| format!("← {}", p.display()));
                (code(k).to_string(), color, detail)
            }
            StatusEntry::Conflicted { kind, .. } => {
                ("U".into(), t.error(), Some(conflict_label(*kind).into()))
            }
            StatusEntry::Untracked { .. } => ("?".into(), widgets::change_color(t, '?'), None),
            StatusEntry::Ignored { .. } => ("!".into(), t.dim, None),
        };
        let path = e.path();
        let mut line =
            widgets::file_entry(t, &code_text, color, path, entry_counts(lines, path, group));
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
            Message::Select(path, Group::Conflicts) => {
                self.selected = Some((path.clone(), Group::Conflicts));
                Update::output(Output::ResolveConflict(path))
            }
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

        let cols = snap
            .status
            .entries
            .iter()
            .map(|e| {
                let extra = match e {
                    StatusEntry::Changed {
                        orig_path: Some(o), ..
                    } => columns(&o.display().to_string()) + 3,
                    StatusEntry::Conflicted { .. } => 16,
                    _ => 0,
                };
                // Widest of the groups the entry can appear in.
                let counts = entry_counts(&snap.lines, e.path(), Group::Staged)
                    .or_else(|| entry_counts(&snap.lines, e.path(), Group::Unstaged))
                    .or_else(|| entry_counts(&snap.lines, e.path(), Group::Untracked));
                widgets::file_entry_columns(e.path(), counts) + extra
            })
            .max()
            .unwrap_or(0);
        // two action icons, the entry and padding.
        let content_width = 64.0 + t.cols(cols) + 24.0;
        let list = virtual_list(
            LIST_ID,
            rows.len(),
            row_height(t),
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
                WtRow::Entry(e, group) => self.entry_row(t, e, group, &snap.lines),
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

    #[test]
    fn arrows_skip_headers_and_space_stages() {
        let t = crate::theme::preset("gf-dark");
        let entries = vec![
            changed("s", ChangeKind::Modified, ChangeKind::Unmodified),
            changed("u", ChangeKind::Unmodified, ChangeKind::Modified),
            StatusEntry::Untracked { path: "n".into() },
        ];
        let mut s = State::default();
        let msg = |s: &State, k| s.nav(Nav::plain(k), &entries, &t).and_then(|m| m.message);
        // Nothing selected: ↓ picks the first file, not the STAGED header.
        assert!(
            matches!(msg(&s, Key::Down), Some(Message::Select(p, Group::Staged)) if p.as_path() == std::path::Path::new("s"))
        );
        let _ = s.update(Message::Select("s".into(), Group::Staged));
        assert!(
            matches!(msg(&s, Key::Down), Some(Message::Select(p, Group::Unstaged)) if p.as_path() == std::path::Path::new("u"))
        );
        assert!(
            matches!(msg(&s, Key::Space), Some(Message::Unstage(p)) if p.as_path() == std::path::Path::new("s"))
        );
        let _ = s.update(Message::Select("n".into(), Group::Untracked));
        assert!(matches!(msg(&s, Key::Space), Some(Message::Stage(_))));
        assert!(matches!(
            msg(&s, Key::Char('d')),
            Some(Message::Discard(_, Group::Untracked))
        ));
        assert!(msg(&s, Key::Down).is_none(), "already last: stays");
    }
}
