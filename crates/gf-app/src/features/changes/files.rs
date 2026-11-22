//! One path per row in the working tree list or directory tree.

use std::collections::BTreeMap;
use std::sync::Arc;

use iced::Length;
use iced::widget::row;

use super::*;
use crate::widgets::virtual_list::{columns, virtual_list};

/// The stage checkbox, or the one-glyph resolve button of a conflict,
/// aligned across groups.
pub(super) fn action_width(t: &Tokens) -> f32 {
    t.font_size.max(t.cols(1) + 10.0)
}

pub(super) fn groups(entry: &StatusEntry) -> [Option<Group>; 2] {
    match entry {
        StatusEntry::Conflicted { .. } => [Some(Group::Conflicts), None],
        StatusEntry::Untracked { .. } => [Some(Group::Untracked), None],
        StatusEntry::Ignored { .. } => [None, None],
        StatusEntry::Changed { .. } => [
            entry.is_staged().then_some(Group::Staged),
            entry.has_unstaged_changes().then_some(Group::Unstaged),
        ],
    }
}

pub(super) fn file_group(entry: &StatusEntry) -> Group {
    match entry {
        StatusEntry::Conflicted { .. } => Group::Conflicts,
        StatusEntry::Untracked { .. } => Group::Untracked,
        _ if entry.has_unstaged_changes() => Group::Unstaged,
        _ => Group::Staged,
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct File {
    pub(super) entry: usize,
    pub(super) staged: bool,
    pub(super) unstaged: bool,
}

#[derive(Debug, Default)]
pub struct FileLists {
    files: Vec<File>,
    tree: file_tree::Tree,
    can_stage: bool,
    can_unstage: bool,
}

impl FileLists {
    /// Index files by path so staging does not move a row. A path may have
    /// separate entries when it was deleted in the index and recreated.
    pub fn new(entries: &[StatusEntry]) -> Self {
        let mut files = BTreeMap::new();
        for (i, entry) in entries.iter().enumerate() {
            if matches!(entry, StatusEntry::Ignored { .. }) {
                continue;
            }
            let file = files.entry(entry.path().as_path()).or_insert(File {
                entry: i,
                staged: false,
                unstaged: false,
            });
            if entry.has_unstaged_changes() {
                file.entry = i;
            }
            file.staged |= entry.is_staged();
            file.unstaged |= entry.has_unstaged_changes();
        }
        let tree = file_tree::Tree::new(files.keys().copied().enumerate());
        let can_stage = files.values().any(|file| file.unstaged);
        let can_unstage = files.values().any(|file| file.staged);
        Self {
            files: files.into_values().collect(),
            tree,
            can_stage,
            can_unstage,
        }
    }

    /// Count each changed path once, including partially staged files.
    pub fn changed_count(&self) -> usize {
        self.files.len()
    }

    pub(super) fn find(&self, entries: &[StatusEntry], path: &Path) -> Option<File> {
        let i = self
            .files
            .binary_search_by(|file| entries[file.entry].path().as_path().cmp(path))
            .ok()?;
        Some(self.files[i])
    }
}

/// Marked files sorted by what each action would do to them.
#[derive(Debug, Default)]
pub(super) struct Marked {
    pub(super) stage: Vec<PathBuf>,
    pub(super) unstage: Vec<PathBuf>,
    pub(super) stash: Vec<PathBuf>,
    /// Changed files `discard` restores.
    pub(super) tracked: Vec<PathBuf>,
    /// New files `discard` deletes.
    pub(super) untracked: Vec<PathBuf>,
}

impl Marked {
    pub(super) fn can_discard(&self) -> bool {
        !self.tracked.is_empty() || !self.untracked.is_empty()
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Row {
    File(usize, Option<usize>),
    Folder(usize),
}

impl State {
    fn visible(&self, lists: &Arc<FileLists>, tree: bool) -> Rc<[Row]> {
        let key = (memo::Same(lists.clone()), tree, self.folders.revision);
        self.visible.get(key, || {
            if tree {
                lists
                    .tree
                    .visible(|path| self.folders.open((), path))
                    .map(|i| match *lists.tree.row(i) {
                        file_tree::Row::Folder { .. } => Row::Folder(i),
                        file_tree::Row::File { index, depth } => Row::File(index, Some(depth)),
                    })
                    .collect()
            } else {
                (0..lists.files.len()).map(|i| Row::File(i, None)).collect()
            }
        })
    }

    /// ↑/↓ stop at files and folded folders only; Enter on a folder toggles
    /// it and `-` folds the folder around the current row.
    pub fn nav(&self, n: Nav, snap: &Snapshot, t: &Tokens, tree: bool) -> Option<Moved<Message>> {
        if n.key == Key::Char('t') {
            return Some(Moved::message(Message::SetTree(!tree)));
        }
        let lists = &snap.change_files;
        let entries = &snap.status.entries;
        let visible = self.visible(lists, tree);
        let focused = self.folders.focused.as_ref().filter(|_| tree);
        let current = visible.iter().position(|&row| match row {
            Row::Folder(index) => matches!(lists.tree.row(index),
                file_tree::Row::Folder { path, .. } if focused.is_some_and(|(_, p)| p == path)),
            Row::File(index, _) => {
                focused.is_none() && self.is_selected_path(entries[lists.files[index].entry].path())
            }
        });
        let selected = current.map(|i| visible[i]);
        if let Some(Row::Folder(index)) = selected {
            if n.key == Key::Enter
                && let file_tree::Row::Folder { path, .. } = lists.tree.row(index)
            {
                return Some(Moved::message(Message::ToggleFolder(path.clone())));
            }
            if matches!(n.key, Key::Space | Key::Char('.' | 'd')) {
                return Some(Moved::stay());
            }
        }
        if !self.marks.is_empty() && matches!(n.key, Key::Space | Key::Char('.' | 'd')) {
            let marked = self.marked(snap);
            let message = match n.key {
                Key::Char('.') => Some(Message::OpenActions),
                Key::Space if !marked.stage.is_empty() => {
                    Some(Message::Act(Output::Stage(marked.stage)))
                }
                Key::Space if !marked.unstage.is_empty() => {
                    Some(Message::Act(Output::Unstage(marked.unstage)))
                }
                Key::Char('d') if marked.can_discard() => Some(Message::Act(Output::Discard {
                    tracked: marked.tracked,
                    untracked: marked.untracked,
                })),
                _ => None,
            };
            return Some(message.map_or_else(Moved::stay, Moved::message));
        }
        if let Some(Row::File(index, _)) = selected {
            let file = lists.files[index];
            let entry = &entries[file.entry];
            let path = entry.path();
            let group = file_group(entry);
            let action = match n.key {
                Key::Space if group == Group::Conflicts => return Some(Moved::stay()),
                Key::Space => Some(self.stage_message(path, file)),
                Key::Char('.') => Some(Message::OpenActions),
                Key::Char('d') => match group {
                    Group::Unstaged | Group::Untracked => {
                        Some(Message::Discard(path.clone(), group))
                    }
                    Group::Conflicts | Group::Staged => return Some(Moved::stay()),
                },
                _ => None,
            };
            if let Some(action) = action {
                return Some(Moved::message(action));
            }
        }
        let h = row_height(t);
        let open = |path: &Path| self.folders.open((), path);
        let tree_row = |row: Row| match row {
            Row::Folder(index) => Some(index),
            Row::File(index, _) => lists.tree.file_row(index),
        };
        if tree && n.key == Key::Char('-') {
            let moved =
                selected
                    .and_then(tree_row)
                    .and_then(|row| lists.tree.fold_target(row, open))
                    .and_then(|target| {
                        let file_tree::Row::Folder { path, .. } = lists.tree.row(target) else {
                            return None;
                        };
                        let at = visible
                            .iter()
                            .position(|&row| matches!(row, Row::Folder(i) if i == target))?;
                        Some(
                            Moved::message(Message::ToggleFolder(path.clone()))
                                .with_task(nav::reveal(LIST_ID, self.scroll, at, h)),
                        )
                    });
            return Some(moved.unwrap_or_else(Moved::stay));
        }
        // ⇧ extends the marked range, so it lands on files only.
        let stop = |i: usize| match visible[i] {
            Row::Folder(index) => !n.shift && lists.tree.is_stop(index, open),
            Row::File(..) => true,
        };
        let row = nav::step_to(
            current,
            visible.len(),
            n.key,
            nav::page(self.scroll, h),
            stop,
        )?;
        if current == Some(row) {
            return Some(Moved::stay());
        }
        let message = match visible[row] {
            Row::File(index, _) => {
                let entry = &entries[lists.files[index].entry];
                let (path, group) = (entry.path().clone(), file_group(entry));
                if n.shift {
                    Message::SelectRange(path, group)
                } else {
                    Message::Select(path, group)
                }
            }
            Row::Folder(index) => {
                let file_tree::Row::Folder { path, .. } = lists.tree.row(index) else {
                    return None;
                };
                Message::FocusFolder(path.clone())
            }
        };
        Some(Moved::message(message).with_task(nav::reveal(LIST_ID, self.scroll, row, h)))
    }

    /// What each action would touch among the marked files. Conflicts are
    /// left out: they are resolved one by one.
    pub(super) fn marked(&self, snap: &Snapshot) -> Marked {
        let lists = &snap.change_files;
        let entries = &snap.status.entries;
        let mut marked = Marked::default();
        for path in &self.marks {
            let Some(file) = lists.find(entries, path) else {
                continue;
            };
            match &entries[file.entry] {
                StatusEntry::Conflicted { .. } | StatusEntry::Ignored { .. } => {}
                StatusEntry::Untracked { .. } => {
                    marked.stage.push(path.clone());
                    marked.untracked.push(path.clone());
                }
                StatusEntry::Changed { .. } => {
                    if file.unstaged {
                        marked.stage.push(path.clone());
                        marked.tracked.push(path.clone());
                    }
                    if file.staged {
                        marked.unstage.push(path.clone());
                    }
                    marked.stash.push(path.clone());
                }
            }
        }
        marked
    }

    /// Files listed from the anchor (or the selection) to `target`, in the
    /// order shown.
    pub(super) fn range(
        &self,
        snap: &Snapshot,
        tree: bool,
        target: &Path,
    ) -> std::collections::BTreeSet<PathBuf> {
        let lists = &snap.change_files;
        let entries = &snap.status.entries;
        let shown: Vec<&PathBuf> = self
            .visible(lists, tree)
            .iter()
            .filter_map(|row| match *row {
                Row::File(index, _) => Some(entries[lists.files[index].entry].path()),
                Row::Folder(_) => None,
            })
            .collect();
        let anchor = self
            .mark_anchor
            .as_deref()
            .or(self.selected.as_ref().map(|(p, _)| p.as_path()))
            .unwrap_or(target);
        let at = |p: &Path| shown.iter().position(|s| s.as_path() == p);
        let Some(end) = at(target) else {
            return std::iter::once(target.to_path_buf()).collect();
        };
        let start = at(anchor).unwrap_or(end);
        shown[start.min(end)..=start.max(end)]
            .iter()
            .map(|p| (*p).clone())
            .collect()
    }

    /// Actions on every marked file at once.
    pub(super) fn marked_menu(&self, snap: &Snapshot) -> Menu<Message> {
        let marked = self.marked(snap);
        let act = |o: Output| Message::Act(o);
        let mut entries = Vec::new();
        if !marked.stage.is_empty() {
            entries.push(Entry::item(
                's',
                format!("stage ({})", marked.stage.len()),
                "git add -- <files>",
                act(Output::Stage(marked.stage.clone())),
            ));
        }
        if !marked.unstage.is_empty() {
            entries.push(Entry::item(
                'u',
                format!("unstage ({})", marked.unstage.len()),
                "git restore --staged -- <files>",
                act(Output::Unstage(marked.unstage.clone())),
            ));
        }
        if !marked.stash.is_empty() {
            entries.push(Entry::item(
                'z',
                format!("stash these files ({})", marked.stash.len()),
                "git stash push -- <files>",
                act(Output::StashFiles(marked.stash.clone())),
            ));
        }
        if marked.can_discard() {
            let n = marked.tracked.len() + marked.untracked.len();
            entries.push(Entry::caution(
                'd',
                format!("discard changes… ({n})"),
                "restore changed files, delete new ones",
                act(Output::Discard {
                    tracked: marked.tracked,
                    untracked: marked.untracked,
                }),
            ));
        }
        if !entries.is_empty() {
            entries.push(Entry::Separator);
        }
        entries.push(Entry::item(
            'c',
            "clear selection",
            "unmark the selected files (Esc)",
            Message::ClearMarks,
        ));
        Menu {
            title: format!("{} files selected", self.marks.len()),
            entries,
        }
    }

    pub(super) fn files_view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let mode = file_tree::mode_button(t, cx.file_tree, Message::SetTree(!cx.file_tree));
        let Some(snap) = cx.snapshot else {
            return widgets::panel_with_action(
                t,
                "WORKING TREE".into(),
                mode,
                widgets::empty_hint(t, "loading…"),
            );
        };
        let lists = &snap.change_files;
        let total = lists.changed_count();
        if total == 0 {
            return widgets::panel_with_action(
                t,
                "WORKING TREE".into(),
                mode,
                widgets::empty_hint(t, "✓ clean — edit files and they appear here"),
            );
        }
        let visible = self.visible(lists, cx.file_tree);
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
                    let counts = [Group::Staged, Group::Unstaged, Group::Untracked]
                        .into_iter()
                        .map(|group| {
                            widgets::file_entry_columns(
                                e.path(),
                                entry_counts(&snap.lines, e.path(), group),
                            )
                        })
                        .max()
                        .unwrap_or(0);
                    counts + extra
                })
                .max()
                .unwrap_or(0)
        });
        let extra = if cx.file_tree { 10 } else { 0 };
        let list = virtual_list(
            LIST_ID,
            visible.len(),
            row_height(t),
            88.0 + t.cols(cols + extra),
            self.scroll,
            move |i| match visible[i] {
                Row::File(index, depth) => {
                    let file = lists.files[index];
                    self.entry_row(
                        t,
                        &snap.status.entries[file.entry],
                        file,
                        &snap.lines,
                        depth,
                        cx.staging,
                    )
                }
                Row::Folder(index) => {
                    let file_tree::Row::Folder {
                        path,
                        label,
                        depth,
                        count,
                        ..
                    } = lists.tree.row(index)
                    else {
                        return widgets::empty_hint(t, "");
                    };
                    row![
                        space().width(Length::Fixed(t.cols(1) + action_width(t) + 6.0)),
                        file_tree::folder(
                            t,
                            label,
                            *depth,
                            *count,
                            self.folders.open((), path),
                            self.folders
                                .focused
                                .as_ref()
                                .is_some_and(|(_, p)| p == path),
                            Message::ToggleFolder(path.clone()),
                        ),
                    ]
                    .into()
                }
            },
            Message::Scrolled,
        );
        let busy = cx.staging || !self.pending_stage.is_empty();
        let actions = row![
            widgets::icon_button_maybe(
                t,
                "+",
                t.added(),
                "stage all changes",
                (lists.can_stage && !busy).then_some(Message::StageAll)
            ),
            widgets::icon_button_maybe(
                t,
                "−",
                t.removed(),
                "unstage all changes",
                (lists.can_unstage && !busy).then_some(Message::UnstageAll)
            ),
            mode,
        ]
        .align_y(iced::Alignment::Center);
        widgets::panel_with_action(t, format!("WORKING TREE ({total})"), actions, list)
    }
}
