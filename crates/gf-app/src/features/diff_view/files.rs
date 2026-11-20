//! Commit file navigation and display, including directory grouping.

use std::ffi::OsStr;
use std::path::Path;

use super::*;

pub(super) fn display_path(file: &FileDiff) -> &Path {
    file.display_path().map_or(Path::new(""), PathBuf::as_path)
}

pub(super) fn original_name(file: &FileDiff) -> Option<&OsStr> {
    match file.change {
        FileChange::Renamed { .. } | FileChange::Copied { .. } => {
            file.old_path.as_deref()?.file_name()
        }
        _ => None,
    }
}

fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.7).round()
}

impl State {
    fn visible_files(&self) -> Rc<[usize]> {
        let key = (memo::Same(self.diff.clone()), self.folders.revision);
        self.visible_files.get(key, || {
            self.diff
                .file_tree
                .visible(|path| self.folders.open((), path))
                .collect()
        })
    }

    /// Move between visible folders and files, or toggle the focused folder.
    pub fn files_nav(&self, n: Nav, t: &Tokens, tree: bool) -> Option<Moved<Message>> {
        if n.key == Key::Char('t') {
            return Some(Moved::message(Message::SetTree(!tree)));
        }
        let visible = tree.then(|| self.visible_files());
        let focused = self.folders.focused.as_ref();
        let current = if let Some(rows) = &visible {
            rows.iter()
                .position(|&row| match self.diff.file_tree.row(row) {
                    file_tree::Row::Folder { path, .. } => focused.is_some_and(|(_, p)| p == path),
                    file_tree::Row::File { index, .. } => focused.is_none() && *index == self.file,
                })
        } else {
            (!self.diff.files.is_empty()).then_some(self.file)
        };
        let folder = current.zip(visible.as_ref()).and_then(|(i, rows)| {
            match self.diff.file_tree.row(rows[i]) {
                file_tree::Row::Folder { path, .. } => Some(path),
                file_tree::Row::File { .. } => None,
            }
        });
        if n.key == Key::Enter
            && let Some(path) = folder
        {
            return Some(Moved::message(Message::ToggleFolder(path.clone())));
        }
        if n.key == Key::Char('.') {
            return Some(if current.is_some() && folder.is_none() {
                Moved::message(Message::OpenFileMenu)
            } else {
                Moved::stay()
            });
        }
        let h = row_height(t);
        let len = visible
            .as_ref()
            .map_or(self.diff.files.len(), |rows| rows.len());
        let i = nav::step(current, len, n.key, nav::page(self.files_scroll, h))?;
        if current == Some(i) {
            return Some(Moved::stay());
        }
        let msg = if let Some(rows) = visible {
            match self.diff.file_tree.row(rows[i]) {
                file_tree::Row::Folder { path, .. } => Message::FocusFolder(path.clone()),
                file_tree::Row::File { index, .. } => Message::SelectFile(*index),
            }
        } else {
            Message::SelectFile(i)
        };
        Some(Moved::message(msg).with_task(nav::reveal(FILES_ID, self.files_scroll, i, h)))
    }

    fn file_row<'a>(&self, t: &'a Tokens, i: usize, depth: Option<usize>) -> Element<'a, Message> {
        let file = &self.diff.files[i];
        let code = match file.change {
            FileChange::Added => "A",
            FileChange::Deleted => "D",
            FileChange::Modified => "M",
            FileChange::Renamed { .. } => "R",
            FileChange::Copied { .. } => "C",
        };
        let path = display_path(file);
        let color = widgets::change_color(t, code.chars().next().unwrap_or(' '));
        let counts = self.diff.entry_counts[i];
        let line = if let Some(depth) = depth {
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy();
            let name = original_name(file).map_or_else(
                || name.to_string(),
                |old| format!("{} → {name}", old.to_string_lossy()),
            );
            let entry = widgets::file_entry_details(t, format!("/{name}"), None, counts);
            row![
                colored(t, code, color),
                iced::widget::space().width(file_tree::indent(t, depth)),
                entry
            ]
            .spacing(3)
            .align_y(iced::Alignment::Center)
        } else {
            widgets::file_entry(t, code, color, path, counts)
        };
        let item = iced::widget::mouse_area(widgets::list_row(
            t,
            line,
            i == self.file,
            Message::SelectFile(i),
        ))
        .on_right_press(Message::FileMenu(i));
        let tip = match (&file.change, &file.old_path) {
            (FileChange::Renamed { .. } | FileChange::Copied { .. }, Some(old)) => {
                format!("{} → {}", old.display(), path.display())
            }
            _ => path.display().to_string(),
        };
        widgets::tip(t, item, tip)
    }

    pub fn files_view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let files = &self.diff.files;
        let title = if files.is_empty() {
            "FILES".into()
        } else {
            format!("FILES ({})", files.len())
        };
        let body = if self.source.is_none() {
            widgets::empty_hint(t, "pick a commit ↑")
        } else if self.loading && files.is_empty() {
            widgets::empty_hint(t, "loading…")
        } else if files.is_empty() {
            widgets::empty_hint(t, "no file changes")
        } else {
            let visible = cx.file_tree.then(|| self.visible_files());
            let (len, columns) = visible
                .as_ref()
                .map_or((files.len(), self.diff.file_list_columns), |rows| {
                    (rows.len(), self.diff.file_tree_columns)
                });
            virtual_list(
                FILES_ID,
                len,
                row_height(t),
                t.cols(columns) + 30.0,
                self.files_scroll,
                move |i| match &visible {
                    None => self.file_row(t, i, None),
                    Some(rows) => match self.diff.file_tree.row(rows[i]) {
                        file_tree::Row::File { index, depth } => {
                            self.file_row(t, *index, Some(*depth))
                        }
                        file_tree::Row::Folder {
                            path,
                            label,
                            depth,
                            count,
                            ..
                        } => row![
                            iced::widget::space().width(Length::Fixed(t.cols(1) + 6.0)),
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
                        .into(),
                    },
                },
                Message::FilesScrolled,
            )
        };
        widgets::accent_panel_with_action(
            t,
            title,
            self.role.accent(t),
            file_tree::mode_button(t, cx.file_tree, Message::SetTree(!cx.file_tree)),
            body,
        )
    }
}
