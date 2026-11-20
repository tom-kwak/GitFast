//! File groups and their visible rows in the working tree pane.

use std::sync::Arc;

use iced::Length;
use iced::widget::{container, row};

use super::*;
use crate::widgets::colored;
use crate::widgets::virtual_list::{columns, virtual_list};

const GROUPS: [Group; 4] = [
    Group::Conflicts,
    Group::Staged,
    Group::Unstaged,
    Group::Untracked,
];

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

#[derive(Debug, Default)]
pub struct FileLists {
    indices: [Vec<usize>; 4],
    trees: [file_tree::Tree; 4],
    changed_count: usize,
}

impl FileLists {
    /// Build the group indices and trees when a new Git status arrives.
    pub fn new(entries: &[StatusEntry]) -> Self {
        let mut indices: [Vec<usize>; 4] = Default::default();
        let mut changed_count = 0;
        for (index, entry) in entries.iter().enumerate() {
            changed_count += usize::from(!matches!(entry, StatusEntry::Ignored { .. }));
            for group in groups(entry).into_iter().flatten() {
                indices[group as usize].push(index);
            }
        }
        let trees = std::array::from_fn(|group| {
            file_tree::Tree::new(
                indices[group]
                    .iter()
                    .map(|&i| (i, entries[i].path().as_path())),
            )
        });
        Self {
            indices,
            trees,
            changed_count,
        }
    }

    /// Count each changed path once, including partially staged files.
    pub fn changed_count(&self) -> usize {
        self.changed_count
    }

    pub(super) fn indices(&self, group: Group) -> &[usize] {
        &self.indices[group as usize]
    }

    fn tree(&self, group: Group) -> &file_tree::Tree {
        &self.trees[group as usize]
    }
}

#[derive(Debug, Clone, Copy)]
enum Row {
    Header(Group),
    File(Group, usize, Option<usize>),
    Folder(Group, usize),
}

#[derive(Debug, Default)]
pub(super) struct Visible {
    rows: Vec<Row>,
    focusable: Vec<usize>,
}

impl Visible {
    fn push(&mut self, row: Row) {
        if !matches!(row, Row::Header(_)) {
            self.focusable.push(self.rows.len());
        }
        self.rows.push(row);
    }
}

impl State {
    fn visible(&self, lists: &Arc<FileLists>, tree: bool) -> Rc<Visible> {
        let key = (memo::Same(lists.clone()), tree, self.folders.revision);
        self.visible.get(key, || {
            let mut visible = Visible::default();
            for group in GROUPS {
                let files = lists.indices(group);
                if files.is_empty() {
                    continue;
                }
                visible.push(Row::Header(group));
                if tree {
                    let tree = lists.tree(group);
                    for i in tree.visible(|path| self.folders.open(group, path)) {
                        visible.push(match *tree.row(i) {
                            file_tree::Row::Folder { .. } => Row::Folder(group, i),
                            file_tree::Row::File { index, depth } => {
                                Row::File(group, index, Some(depth))
                            }
                        });
                    }
                } else {
                    for &i in files {
                        visible.push(Row::File(group, i, None));
                    }
                }
            }
            Rc::new(visible)
        })
    }

    /// Folder rows participate in movement, and Enter expands or folds them.
    pub fn nav(&self, n: Nav, snap: &Snapshot, t: &Tokens, tree: bool) -> Option<Moved<Message>> {
        if n.key == Key::Char('t') {
            return Some(Moved::message(Message::SetTree(!tree)));
        }
        let lists = &snap.change_files;
        let entries = &snap.status.entries;
        let visible = self.visible(lists, tree);
        let focused = self.folders.focused.as_ref().filter(|_| tree);
        let current = visible.focusable.iter().position(|&row| match visible.rows[row] {
            Row::Folder(group, index) => matches!(lists.tree(group).row(index),
                file_tree::Row::Folder { path, .. } if focused.is_some_and(|(g, p)| *g == group && p == path)),
            Row::File(group, index, _) => focused.is_none() && self.is_selected(entries[index].path(), group),
            Row::Header(_) => false,
        });
        let selected = current.map(|i| visible.rows[visible.focusable[i]]);
        if let Some(Row::Folder(group, index)) = selected {
            if n.key == Key::Enter
                && let file_tree::Row::Folder { path, .. } = lists.tree(group).row(index)
            {
                return Some(Moved::message(Message::ToggleFolder(group, path.clone())));
            }
            if matches!(n.key, Key::Space | Key::Char('.' | 'd')) {
                return Some(Moved::stay());
            }
        }
        if let Some(Row::File(group, index, _)) = selected {
            let path = entries[index].path();
            let action = match n.key {
                Key::Space => match group {
                    Group::Staged => Some(Message::Unstage(path.clone())),
                    Group::Unstaged | Group::Untracked => Some(Message::Stage(path.clone())),
                    Group::Conflicts => return Some(Moved::stay()),
                },
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
        let next = nav::step(
            current,
            visible.focusable.len(),
            n.key,
            nav::page(self.scroll, h),
        )?;
        if current == Some(next) {
            return Some(Moved::stay());
        }
        let row = visible.focusable[next];
        let message = match visible.rows[row] {
            Row::File(group, index, _) => Message::Select(entries[index].path().clone(), group),
            Row::Folder(group, index) => {
                let file_tree::Row::Folder { path, .. } = lists.tree(group).row(index) else {
                    return None;
                };
                Message::FocusFolder(group, path.clone())
            }
            Row::Header(_) => return None,
        };
        Some(Moved::message(message).with_task(nav::reveal(LIST_ID, self.scroll, row, h)))
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
        let total = GROUPS
            .iter()
            .map(|&g| lists.indices(g).len())
            .sum::<usize>();
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
            visible.rows.len(),
            row_height(t),
            88.0 + t.cols(cols + extra),
            self.scroll,
            move |i| match visible.rows[i] {
                Row::Header(group) => header(t, group, lists.indices(group).len(), cx.staging),
                Row::File(group, index, depth) => self.entry_row(
                    t,
                    &snap.status.entries[index],
                    group,
                    &snap.lines,
                    depth,
                    cx.staging,
                ),
                Row::Folder(group, index) => {
                    let file_tree::Row::Folder {
                        path, depth, count, ..
                    } = lists.tree(group).row(index)
                    else {
                        return widgets::empty_hint(t, "");
                    };
                    file_tree::folder(
                        t,
                        path,
                        *depth,
                        *count,
                        self.folders.open(group, path),
                        self.folders
                            .focused
                            .as_ref()
                            .is_some_and(|(g, p)| *g == group && p == path),
                        Message::ToggleFolder(group, path.clone()),
                    )
                }
            },
            Message::Scrolled,
        );
        widgets::panel_with_action(t, format!("WORKING TREE ({total})"), mode, list)
    }
}

fn header(t: &Tokens, group: Group, count: usize, staging: bool) -> Element<'_, Message> {
    let (title, color, action) = match group {
        Group::Conflicts => ("CONFLICTS", t.error(), None),
        Group::Staged => (
            "STAGED",
            t.added(),
            Some(("unstage all", Message::UnstageAll)),
        ),
        Group::Unstaged => (
            "UNSTAGED",
            t.removed(),
            Some(("stage all", Message::StageAll)),
        ),
        Group::Untracked => ("UNTRACKED", t.dim, None),
    };
    let mut header = row![colored(t, format!("{title} ({count})"), color).size(t.small())]
        .spacing(8)
        .align_y(iced::Alignment::Center);
    if let Some((label, message)) = action {
        header = header.push(widgets::text_button(
            t,
            label,
            None,
            (!staging).then_some(message),
        ));
    }
    container(header)
        .height(Length::Fill)
        .align_y(iced::Alignment::Center)
        .into()
}
