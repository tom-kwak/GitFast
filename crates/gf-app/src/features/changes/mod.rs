//! Working tree files with staging checkboxes, diff selection, and file actions.

mod files;
pub use files::FileLists;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gf_core::model::{ChangeKind, ConflictKind, StatusEntry};
use gf_git::FileVersion;
use iced::widget::text::Wrapping;
use iced::widget::{checkbox, container, row, space};
use iced::{Color, Element, Length};

use crate::feature::{Cx, Feature, Update};
use crate::features::context_menu::{Entry, Menu};
use crate::features::diff_view::DiffSource;
use crate::nav::{self, Key, Moved, Nav};
use crate::repo_tab::{LineStats, Snapshot};
use crate::theme::Tokens;
use crate::widgets::file_tree;
use crate::widgets::memo::{self, Memo};
use crate::widgets::virtual_list::ScrollPos;
use crate::widgets::{self, dim};

/// The diff or resolver to open for the selected file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    folders: file_tree::Folders<()>,
    visible: Memo<(memo::Same<FileLists>, bool, u64), Rc<[files::Row]>>,
    pending_stage: HashMap<PathBuf, bool>,
    partial_menu: Option<Group>,
    /// Widest entry of the status shown, in columns.
    widest: Memo<memo::Id, usize>,
    /// Whether the current menu's file is absent from HEAD.
    menu_new_file: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(PathBuf, Group),
    FocusFolder(PathBuf),
    ToggleFolder(PathBuf),
    SetTree(bool),
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
    SetTree(bool),
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
    ViewFile(PathBuf, Group),
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
    entries.push(Entry::item(
        'v',
        "view file",
        "show the entire file",
        act(Output::ViewFile(pb.clone(), group)),
    ));
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

/// A partly staged file has a different diff in each group.
pub(crate) fn diff_source(path: PathBuf, group: Group) -> DiffSource {
    match group {
        Group::Staged => DiffSource::Staged(path),
        Group::Untracked => DiffSource::Untracked(path),
        Group::Conflicts | Group::Unstaged => DiffSource::WorkTree(path),
    }
}

impl State {
    /// Whether this is the file and diff already open in the pane.
    pub(crate) fn is_selected(&self, path: &Path, group: Group) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|(p, g)| p == path && *g == group)
    }

    fn is_selected_path(&self, path: &Path) -> bool {
        self.selected.as_ref().is_some_and(|(p, _)| p == path)
    }

    pub(crate) fn set_pending_stage(&mut self, path: PathBuf, checked: bool) {
        self.pending_stage.insert(path, checked);
    }

    pub(crate) fn clear_pending_stage(&mut self) {
        self.pending_stage.clear();
    }

    pub(crate) fn forget_pending_stage(&mut self, path: &Path) {
        self.pending_stage.remove(path);
    }

    fn stage_message(&self, path: &Path, file: files::File) -> Message {
        let checked = self
            .pending_stage
            .get(path)
            .copied()
            .unwrap_or(file.staged && !file.unstaged);
        if checked {
            Message::Unstage(path.to_path_buf())
        } else {
            Message::Stage(path.to_path_buf())
        }
    }

    /// Keeps the selection pointing at something that still exists, moving
    /// its displayed diff when a file was staged/unstaged. Returns the diff
    /// to show if the selection moved.
    pub fn follow(&mut self, entries: &[StatusEntry]) -> Option<DiffSource> {
        let (path, group) = self.selected.clone()?;
        let has = |group| {
            entries
                .iter()
                .any(|e| e.path() == &path && files::groups(e).contains(&Some(group)))
        };
        if has(group) {
            return None;
        }
        let moved = [
            Group::Staged,
            Group::Unstaged,
            Group::Untracked,
            Group::Conflicts,
        ]
        .into_iter()
        .find(|&group| has(group));
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
        file: files::File,
        lines: &LineStats,
        depth: Option<usize>,
        staging: bool,
    ) -> Element<'a, Message> {
        let group = files::file_group(e);
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
        let counts = entry_counts(lines, path, group);
        let mut line = if let Some(depth) = depth {
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy();
            row![
                space().width(file_tree::indent(t, depth)),
                widgets::file_entry_details(t, format!("/{name}"), None, counts),
            ]
            .align_y(iced::Alignment::Center)
        } else {
            widgets::file_entry(t, &code_text, color, path, counts)
        };
        if let Some(d) = detail {
            line = line.push(dim(t, d).wrapping(Wrapping::None));
        }
        let selected = self.is_selected_path(path);
        let main = iced::widget::mouse_area(widgets::list_row(
            t,
            line,
            selected,
            Message::Select(path.clone(), group),
        ))
        .on_right_press(Message::RightClick(path.clone(), group));
        let main: Element<'_, _> = if depth.is_some() {
            widgets::tip(t, main, path.display().to_string())
        } else {
            main.into()
        };

        let partial = file.staged && file.unstaged && !self.pending_stage.contains_key(path);
        let action = self.stage_message(path, file);
        let checked = matches!(action, Message::Unstage(_));
        let control = checkbox(checked || partial)
            .size(t.font_size)
            .style(crate::theme::style::staged_checkbox(t))
            .on_toggle_maybe((!staging).then_some(move |_| action.clone()));
        let control = if partial {
            control.icon(iced::widget::checkbox::Icon {
                font: iced::Font::MONOSPACE,
                code_point: '−',
                size: None,
                line_height: Default::default(),
                shaping: Default::default(),
            })
        } else {
            control
        };
        let toggle = widgets::tip(
            t,
            control,
            if checked {
                "remove these changes from the next commit (Space)"
            } else {
                "include these changes in the next commit (Space)"
            }
            .into(),
        );
        let actions: Element<'a, Message> = match group {
            Group::Staged => toggle,
            Group::Unstaged => row![
                toggle,
                widgets::icon_button(
                    t,
                    "↺",
                    t.dim,
                    "discard changes…",
                    Message::Discard(path.clone(), group)
                ),
            ]
            .into(),
            Group::Untracked => row![
                toggle,
                widgets::icon_button(
                    t,
                    "✕",
                    t.dim,
                    "delete file…",
                    Message::Discard(path.clone(), group)
                ),
            ]
            .into(),
            Group::Conflicts => widgets::icon_button_maybe(
                t,
                "✓",
                t.added(),
                "mark resolved (stage)",
                (!staging).then(|| Message::Stage(path.clone())),
            ),
        };
        // Keep action and status columns outside the indented path.
        let line = if depth.is_some() {
            let status =
                iced::widget::mouse_area(widgets::colored(t, code_text, color).width(t.cols(1)))
                    .on_press(Message::Select(path.clone(), group))
                    .on_right_press(Message::RightClick(path.clone(), group));
            row![
                status,
                container(actions).width(Length::Fixed(files::action_width(t))),
                main
            ]
        } else {
            row![actions, main]
        };
        line.spacing(3).align_y(iced::Alignment::Center).into()
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
        let path = match &message {
            Message::RightClick(path, _) => Some(path),
            _ => self.selected.as_ref().map(|(path, _)| path),
        };
        self.partial_menu = snap.zip(path).and_then(|(snap, path)| {
            let file = snap.change_files.find(&snap.status.entries, path)?;
            (file.staged && file.unstaged)
                .then(|| files::file_group(&snap.status.entries[file.entry]))
        });
        self.menu_new_file = snap.zip(path).is_some_and(|(snap, path)| {
            snap.status.entries.iter().any(|entry| matches!(entry,
                StatusEntry::Changed { path: changed, index: ChangeKind::Added | ChangeKind::Renamed | ChangeKind::Copied, .. }
                    if changed == path))
        });
        self.update(message)
    }

    fn menu(&self, path: &Path, group: Group) -> Menu<Message> {
        let mut menu = file_menu(path, group, !self.menu_new_file);
        if let Some(worktree) = self.partial_menu {
            let (label, side) = if group == Group::Staged {
                ("worktree diff", worktree)
            } else {
                menu.entries.insert(
                    1,
                    Entry::item(
                        'u',
                        "unstage",
                        "remove changes from the index",
                        Message::Unstage(path.to_path_buf()),
                    ),
                );
                ("staged diff", Group::Staged)
            };
            menu.entries.push(Entry::item(
                'x',
                label,
                "show the other side of this file",
                Message::Select(path.to_path_buf(), side),
            ));
        }
        menu
    }
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message {
            Message::SetTree(tree) => {
                self.folders.focused = None;
                self.scroll = ScrollPos::default();
                Update {
                    task: nav::scroll_y(LIST_ID, 0.0),
                    output: Some(Output::SetTree(tree)),
                }
            }
            Message::FocusFolder(path) => {
                self.folders.focused = Some(((), path));
                Update::none()
            }
            Message::ToggleFolder(path) => {
                self.folders.toggle((), path);
                Update::none()
            }
            Message::Select(path, Group::Conflicts) => {
                self.folders.focused = None;
                self.selected = Some((path.clone(), Group::Conflicts));
                Update::output(Output::ResolveConflict(path))
            }
            Message::Select(path, group) => {
                self.folders.focused = None;
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
                self.folders.focused = None;
                let changed = !self.is_selected(&path, group);
                self.selected = Some((path.clone(), group));
                let show = (changed && group != Group::Conflicts)
                    .then(|| diff_source(path.clone(), group));
                Update::output(Output::Menu {
                    show,
                    menu: self.menu(&path, group),
                })
            }
            Message::OpenActions => match self.selected.clone() {
                Some((path, group)) => Update::output(Output::Menu {
                    show: None,
                    menu: self.menu(&path, group),
                }),
                None => Update::none(),
            },
            Message::Act(output) => Update::output(output),
        }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        self.files_view(cx)
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
        assert_eq!(keys(&menu), "szdvhbyf");
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
            "uzvhbyf"
        );
        assert_eq!(
            keys(&file_menu("a.rs".as_ref(), Group::Conflicts, true)),
            "rotsvyf"
        );
        assert_eq!(
            keys(&file_menu("logs/x.log".as_ref(), Group::Untracked, true)),
            "siegdvyf"
        );
        // A file new in the index has no history or blame to show yet.
        assert_eq!(
            keys(&file_menu("a.rs".as_ref(), Group::Staged, false)),
            "uzvyf"
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
    fn partly_staged_file_occupies_one_row() {
        let entries = vec![
            changed("a", ChangeKind::Modified, ChangeKind::Modified),
            StatusEntry::Untracked { path: "b".into() },
            StatusEntry::Ignored { path: "c".into() },
        ];
        let files = FileLists::new(&entries);
        assert_eq!(files.changed_count(), 2);
        let partial = files
            .find(&entries, Path::new("a"))
            .expect("partial file row");
        assert!(partial.staged && partial.unstaged);
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
    fn arrows_follow_sorted_paths_and_space_stages() {
        let t = crate::theme::preset("gf-dark");
        let entries = vec![
            changed("s", ChangeKind::Modified, ChangeKind::Unmodified),
            changed("u", ChangeKind::Unmodified, ChangeKind::Modified),
            StatusEntry::Untracked { path: "n".into() },
        ];
        let mut s = State::default();
        let snap = Snapshot {
            change_files: std::sync::Arc::new(FileLists::new(&entries)),
            status: gf_core::model::StatusReport {
                entries,
                ..Default::default()
            },
            ..Default::default()
        };
        let msg = |s: &State, k| {
            s.nav(Nav::plain(k), &snap, &t, false)
                .and_then(|m| m.message)
        };
        // Nothing selected: ↓ picks the first alphabetical file.
        assert!(
            matches!(msg(&s, Key::Down), Some(Message::Select(p, Group::Untracked)) if p.as_path() == Path::new("n"))
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
        let _ = s.update(Message::Select("u".into(), Group::Unstaged));
        assert!(msg(&s, Key::Down).is_none(), "already last: stays");
    }
}
