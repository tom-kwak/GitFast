//! Working tree changes grouped like `git status` (conflicts, staged,
//! unstaged, untracked) with per-row and per-group actions. Clicking a row
//! shows its diff; right-click (or `.`) opens the file's actions, which
//! depend on its group.

use std::path::{Path, PathBuf};

use gf_core::model::{ChangeKind, ConflictKind, StatusEntry};
use gf_git::FileVersion;
use iced::widget::text::Wrapping;
use iced::widget::{container, row};
use iced::{Color, Element, Length};

use crate::feature::{Cx, Feature, Update};
use crate::features::context_menu::{Entry, Menu};
use crate::features::diff_view::DiffSource;
use crate::nav::{self, Key, Moved, Nav};
use crate::repo_tab::{LineStats, Snapshot};
use crate::theme::Tokens;
use crate::widgets::memo::{self, Memo};
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
    /// Widest entry of the status shown, in columns.
    widest: Memo<memo::Id, usize>,
    /// Files the last commit does not have (set before a menu opens).
    new_files: Vec<PathBuf>,
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
    /// Selects the row and opens its actions.
    RightClick(PathBuf, Group),
    /// `.`: the selected row's actions.
    OpenActions,
    /// A menu action (a request to the app).
    Act(Output),
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
    /// Open the actions menu; `show` is the diff of a newly selected row.
    Menu {
        show: Option<DiffSource>,
        menu: Menu<Message>,
    },
    FileHistory(PathBuf),
    Blame(PathBuf),
    CopyPath(PathBuf),
    Reveal(PathBuf),
    /// Stash only this file's changes.
    StashFile(PathBuf),
    /// Add a line to `.gitignore`.
    Ignore(String),
    TakeSide(PathBuf, gf_git::Side),
}

/// `.gitignore` lines offered for an untracked `path`: the file itself,
/// every file with its extension, and its folder.
pub fn ignore_patterns(path: &std::path::Path) -> Vec<(char, String, String)> {
    let shown = path.display().to_string();
    let mut out = vec![('i', "ignore this file".to_owned(), format!("/{shown}"))];
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        out.push(('e', format!("ignore all *.{ext}"), format!("*.{ext}")));
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        out.push((
            'g',
            "ignore its folder".to_owned(),
            format!("/{}/", dir.display()),
        ));
    }
    out
}

/// The actions for `path` as listed in `group`, with the git command each
/// runs as its hint. `in_head`: the last commit has the file (so it has
/// a history and a blame).
fn file_menu(path: &Path, group: Group, in_head: bool) -> Menu<Message> {
    let p = path.display().to_string();
    let act = |o: Output| Message::Act(o);
    let pb = path.to_path_buf();
    let mut entries = match group {
        Group::Unstaged => vec![
            Entry::item(
                's',
                "stage",
                format!("git add -- {p}"),
                Message::Stage(pb.clone()),
            ),
            Entry::item(
                'z',
                "stash just this file",
                format!("git stash push -- {p}"),
                act(Output::StashFile(pb.clone())),
            ),
            Entry::caution(
                'd',
                "discard changes…",
                format!("git restore -- {p}"),
                Message::Discard(pb.clone(), group),
            ),
        ],
        Group::Staged => vec![
            Entry::item(
                'u',
                "unstage",
                format!("git restore --staged -- {p}"),
                Message::Unstage(pb.clone()),
            ),
            Entry::item(
                'z',
                "stash just this file",
                format!("git stash push -- {p}"),
                act(Output::StashFile(pb.clone())),
            ),
        ],
        Group::Untracked => {
            let mut v = vec![Entry::item(
                's',
                "stage",
                format!("git add -- {p}"),
                Message::Stage(pb.clone()),
            )];
            v.extend(ignore_patterns(path).into_iter().map(|(k, label, pat)| {
                Entry::item(
                    k,
                    label,
                    format!("+ {pat} in .gitignore"),
                    act(Output::Ignore(pat)),
                )
            }));
            v.push(Entry::caution(
                'd',
                "delete file…",
                format!("rm {p}"),
                Message::Discard(pb.clone(), group),
            ));
            v
        }
        Group::Conflicts => vec![
            Entry::item(
                'r',
                "open resolver",
                "pick block by block",
                Message::Select(pb.clone(), group),
            ),
            Entry::item(
                'o',
                "take ours",
                format!("git checkout --ours -- {p}"),
                act(Output::TakeSide(pb.clone(), gf_git::Side::Ours)),
            ),
            Entry::item(
                't',
                "take theirs",
                format!("git checkout --theirs -- {p}"),
                act(Output::TakeSide(pb.clone(), gf_git::Side::Theirs)),
            ),
            Entry::item(
                's',
                "mark resolved",
                format!("git add -- {p}"),
                Message::Stage(pb.clone()),
            ),
        ],
    };
    entries.push(Entry::Separator);
    // A new file has no history or blame yet.
    if in_head && matches!(group, Group::Staged | Group::Unstaged) {
        entries.push(Entry::item(
            'h',
            "file history",
            format!("git log --follow -- {p}"),
            act(Output::FileHistory(pb.clone())),
        ));
        entries.push(Entry::item(
            'b',
            "blame",
            "who changed each line",
            act(Output::Blame(pb.clone())),
        ));
    }
    entries.push(Entry::item(
        'y',
        "copy path",
        p.clone(),
        act(Output::CopyPath(pb.clone())),
    ));
    entries.push(Entry::item(
        'f',
        "reveal in Finder",
        "show it in its folder",
        act(Output::Reveal(pb)),
    ));
    Menu { title: p, entries }
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

/// Identify which file to read for the selected group, including deleted files.
pub fn preview_version(path: &Path, group: Group, snap: Option<&Snapshot>) -> FileVersion {
    let entry = snap.and_then(|s| s.status.entries.iter().find(|e| e.path() == path));
    file_version(group, entry)
}

fn file_version(group: Group, entry: Option<&StatusEntry>) -> FileVersion {
    match (group, entry) {
        (
            Group::Staged,
            Some(StatusEntry::Changed {
                index: ChangeKind::Deleted,
                ..
            }),
        ) => FileVersion::Revision("HEAD".into()),
        (Group::Staged, _)
        | (
            Group::Unstaged,
            Some(StatusEntry::Changed {
                worktree: ChangeKind::Deleted,
                ..
            }),
        ) => FileVersion::Index,
        _ => FileVersion::WorkTree,
    }
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
            Key::Char('.') => {
                return self
                    .selected
                    .as_ref()
                    .map(|_| Moved::message(Message::OpenActions));
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
        let main = iced::widget::mouse_area(widgets::list_row(
            t,
            line,
            selected,
            Message::Select(path.clone(), group),
        ))
        .on_right_press(Message::RightClick(path.clone(), group));

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

impl State {
    /// [`Feature::update`] knowing the repository: the file menus leave
    /// out history and blame for files the last commit does not have yet
    /// (added, or the new name of a rename).
    pub fn update_with(
        &mut self,
        message: Message,
        snap: Option<&Snapshot>,
    ) -> Update<Message, Output> {
        // Only menus need it (not every scroll).
        if !matches!(message, Message::RightClick(..) | Message::OpenActions) {
            return self.update(message);
        }
        self.new_files = snap.map_or_else(Vec::new, |s| {
            s.status
                .entries
                .iter()
                .filter_map(|e| match e {
                    StatusEntry::Changed {
                        path,
                        index: ChangeKind::Added | ChangeKind::Renamed | ChangeKind::Copied,
                        ..
                    } => Some(path.clone()),
                    _ => None,
                })
                .collect()
        });
        self.update(message)
    }

    fn in_head(&self, path: &std::path::Path) -> bool {
        !self.new_files.iter().any(|p| p == path)
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
            Message::RightClick(path, group) => {
                // Right-click selects like a click (the diff follows).
                let sel = (path.clone(), group);
                let changed = self.selected.as_ref() != Some(&sel);
                self.selected = Some(sel);
                let show = (changed && group != Group::Conflicts)
                    .then(|| diff_source(path.clone(), group));
                Update::output(Output::Menu {
                    show,
                    menu: file_menu(&path, group, self.in_head(&path)),
                })
            }
            Message::OpenActions => match self.selected.clone() {
                Some((path, group)) => Update::output(Output::Menu {
                    show: None,
                    menu: file_menu(&path, group, self.in_head(&path)),
                }),
                None => Update::none(),
            },
            Message::Act(output) => Update::output(output),
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

        let key = memo::identity(&snap.status, snap.status.entries.len());
        let cols = self.widest.get(key, || {
            snap.status
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
                .unwrap_or(0)
        });
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

    fn keys(menu: &Menu<Message>) -> String {
        menu.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item { key, .. } => Some(*key),
                Entry::Separator => None,
            })
            .collect()
    }

    #[test]
    fn right_click_selects_and_offers_the_groups_actions() {
        let mut s = State::default();
        let Some(Output::Menu { show, menu }) = s
            .update(Message::RightClick("src/a.rs".into(), Group::Unstaged))
            .output
        else {
            panic!("menu");
        };
        assert_eq!(
            show,
            Some(DiffSource::WorkTree("src/a.rs".into())),
            "diff follows"
        );
        assert_eq!(keys(&menu), "szdhbyf");
        // Again on the same row: nothing new to show.
        let out = s
            .update(Message::RightClick("src/a.rs".into(), Group::Unstaged))
            .output;
        assert!(matches!(out, Some(Output::Menu { show: None, .. })));
        // `.` opens the same menu for the selection.
        assert!(matches!(
            s.update(Message::OpenActions).output,
            Some(Output::Menu { show: None, .. })
        ));

        assert_eq!(
            keys(&file_menu("a.rs".as_ref(), Group::Staged, true)),
            "uzhbyf"
        );
        assert_eq!(
            keys(&file_menu("a.rs".as_ref(), Group::Conflicts, true)),
            "rotsyf"
        );
        assert_eq!(
            keys(&file_menu("logs/x.log".as_ref(), Group::Untracked, true)),
            "siegdyf"
        );
        // A file new in the index has no history or blame to show yet.
        assert_eq!(
            keys(&file_menu("a.rs".as_ref(), Group::Staged, false)),
            "uzyf"
        );
    }

    #[test]
    fn ignore_offers_the_file_its_extension_and_folder() {
        let pats: Vec<String> = ignore_patterns("build/out.o".as_ref())
            .into_iter()
            .map(|(_, _, p)| p)
            .collect();
        assert_eq!(pats, ["/build/out.o", "*.o", "/build/"]);
        let pats: Vec<String> = ignore_patterns("Makefile".as_ref())
            .into_iter()
            .map(|(_, _, p)| p)
            .collect();
        assert_eq!(pats, ["/Makefile"], "no extension, no folder");
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
