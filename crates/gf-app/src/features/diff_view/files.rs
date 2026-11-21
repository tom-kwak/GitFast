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

    /// Whether a file picked by Find File still waits to be scrolled to.
    pub fn take_reveal(&mut self) -> bool {
        std::mem::take(&mut self.reveal_pending)
    }

    /// Scrolls the files list to the selected file, unfolding the folders
    /// that hide it. The list height is unknown until its first scroll, so
    /// the row goes near the top then.
    pub fn reveal_selected<M: Send + 'static>(&mut self, t: &Tokens, tree: bool) -> Task<M> {
        let h = row_height(t);
        let row = if tree {
            let Some((at, folders)) = self.diff.file_tree.locate(self.file) else {
                return Task::none();
            };
            let folders: Vec<PathBuf> = folders.into_iter().map(Path::to_path_buf).collect();
            for folder in folders {
                self.folders.expand((), &folder);
            }
            let visible = self.visible_files();
            let Some(row) = visible.iter().position(|&r| r == at) else {
                return Task::none();
            };
            row
        } else {
            self.file
        };
        if self.files_scroll.height > 0.0 {
            return nav::reveal(FILES_ID, self.files_scroll, row, h);
        }
        #[allow(clippy::cast_precision_loss)]
        let top = row as f32 * h;
        nav::scroll_y(FILES_ID, top - 2.0 * h)
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
            let name = widgets::file_name_label(path);
            let name = original_name(file).map_or_else(
                || name.clone(),
                |old| format!("{} → {name}", old.to_string_lossy()),
            );
            let entry = widgets::file_entry_details(t, name, None, counts);
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
