//! Directory rows shared by the working tree and commit file list.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::hash::Hash;
use std::path::{Path, PathBuf};

use iced::widget::{row, space, text::Wrapping};
use iced::{Element, Length};

use crate::theme::Tokens;
use crate::widgets;

#[derive(Debug, Default)]
pub struct Tree(Vec<Row>);

#[derive(Debug)]
pub enum Row {
    Folder {
        path: PathBuf,
        depth: usize,
        count: usize,
        end: usize,
    },
    File {
        index: usize,
        depth: usize,
    },
}

#[derive(Default)]
struct Directory {
    folders: BTreeMap<OsString, Self>,
    files: BTreeMap<OsString, Vec<usize>>,
    count: usize,
}

impl Directory {
    fn flatten(self, path: &mut PathBuf, depth: usize, rows: &mut Vec<Row>) {
        for (name, directory) in self.folders {
            path.push(name);
            let start = rows.len();
            rows.push(Row::Folder {
                path: path.clone(),
                depth,
                count: directory.count,
                end: 0,
            });
            directory.flatten(path, depth + 1, rows);
            let end = rows.len();
            if let Row::Folder { end: next, .. } = &mut rows[start] {
                *next = end;
            }
            path.pop();
        }
        rows.extend(
            self.files
                .into_values()
                .flatten()
                .map(|index| Row::File { index, depth }),
        );
    }
}

impl Tree {
    /// File indices still refer to the original list, even after sorting.
    pub fn new<'a>(files: impl IntoIterator<Item = (usize, &'a Path)>) -> Self {
        let mut root = Directory::default();
        for (index, path) in files {
            let mut dir = &mut root;
            if let Some(parent) = path.parent() {
                for part in parent.components() {
                    dir = dir.folders.entry(part.as_os_str().into()).or_default();
                    dir.count += 1;
                }
            }
            let name = path.file_name().unwrap_or(path.as_os_str());
            dir.files.entry(name.into()).or_default().push(index);
        }
        let mut rows = Vec::new();
        root.flatten(&mut PathBuf::new(), 0, &mut rows);
        Self(rows)
    }

    pub fn row(&self, index: usize) -> &Row {
        &self.0[index]
    }

    /// Skip whole subtrees when folded, keeping only rows the list can show.
    pub fn visible(&self, mut open: impl FnMut(&Path) -> bool) -> impl Iterator<Item = usize> {
        let mut index = 0;
        std::iter::from_fn(move || {
            let current = index;
            index = match self.0.get(index)? {
                Row::Folder { path, end, .. } if !open(path) => *end,
                _ => index + 1,
            };
            Some(current)
        })
    }
}

/// Fold and keyboard focus are independent for each pane and group.
#[derive(Debug)]
pub struct Folders<G> {
    collapsed: HashMap<G, HashSet<PathBuf>>,
    pub focused: Option<(G, PathBuf)>,
    pub revision: u64,
}

impl<G> Default for Folders<G> {
    fn default() -> Self {
        Self {
            collapsed: HashMap::new(),
            focused: None,
            revision: 0,
        }
    }
}

impl<G: Copy + Eq + Hash> Folders<G> {
    pub fn open(&self, group: G, path: &Path) -> bool {
        !self
            .collapsed
            .get(&group)
            .is_some_and(|paths| paths.contains(path))
    }

    pub fn toggle(&mut self, group: G, path: PathBuf) {
        let paths = self.collapsed.entry(group).or_default();
        if !paths.remove(&path) {
            paths.insert(path.clone());
        }
        self.focused = Some((group, path));
        self.revision = self.revision.wrapping_add(1);
    }
}

/// Two character cells per nesting level.
pub fn indent(t: &Tokens, depth: usize) -> Length {
    Length::Fixed(t.cols(2 * depth))
}

pub fn folder<'a, M: Clone + 'a>(
    t: &'a Tokens,
    path: &Path,
    depth: usize,
    count: usize,
    open: bool,
    focused: bool,
    message: M,
) -> Element<'a, M> {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let line = row![
        space().width(indent(t, depth)),
        widgets::colored(t, if open { "▾ " } else { "▸ " }, t.accent()),
        widgets::mono(t, format!("{name}/")).wrapping(Wrapping::None),
        widgets::dim(t, format!(" ({count})")).wrapping(Wrapping::None),
    ]
    .align_y(iced::Alignment::Center);
    widgets::tip(
        t,
        widgets::list_row(t, line, focused, message),
        "expand or collapse folder ↵".into(),
    )
}

/// Keep the display control compact enough for the narrow file panes.
pub fn mode_button<'a, M: Clone + 'a>(t: &'a Tokens, tree: bool, message: M) -> Element<'a, M> {
    let (label, hint) = if tree {
        ("List", "show flat list (T)")
    } else {
        ("Tree", "show directory tree (T)")
    };
    widgets::tip(
        t,
        widgets::text_button(t, label, None, Some(message)),
        hint.into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folded_subtrees_skip_nested_rows_and_preserve_file_indices() {
        let paths = ["z.md", "src/nested/a.rs", "src/b.rs", "a/a.txt"];
        let tree = Tree::new(paths.into_iter().map(Path::new).enumerate());
        let rows: Vec<_> = tree
            .visible(|path| path != Path::new("src"))
            .map(|index| tree.row(index))
            .collect();
        assert!(matches!(
            rows.as_slice(),
            [
                Row::Folder {
                    depth: 0,
                    count: 1,
                    ..
                },
                Row::File { index: 3, depth: 1 },
                Row::Folder {
                    depth: 0,
                    count: 2,
                    ..
                },
                Row::File { index: 0, depth: 0 },
            ]
        ));
        assert_eq!(tree.visible(|_| true).count(), 7);
    }
}
