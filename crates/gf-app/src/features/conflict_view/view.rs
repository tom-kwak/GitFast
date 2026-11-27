//! Shared scrolling rows keep the inputs and result aligned when reviewing.

use std::collections::HashMap;
use std::path::PathBuf;

use gf_git::Side;
use gf_git::conflict::Choice;
use iced::widget::text::Wrapping;
use iced::widget::{column, container, responsive, row, space, stack, text_editor};
use iced::{Element, Length};

use super::model::{Resolution, Row};
use super::{EDITOR, LIST, Message, State};
use crate::theme::{Tokens, style};
use crate::widgets::{self, dim, mono, virtual_list};

const FILES: &str = "conflict-files";
const BASE: &str = "conflict-base";

pub(super) fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.6).ceil()
}

fn action<'a>(t: &'a Tokens, label: &str, message: Option<Message>) -> Element<'a, Message> {
    widgets::text_button(t, label, None, message)
}

fn cell<'a>(t: &'a Tokens, text: &str, portion: u16, highlight: bool) -> Element<'a, Message> {
    let line = widgets::format::code_line(text).into_owned();
    let text = mono(t, line).wrapping(Wrapping::None);
    container(text)
        .padding([0, 6])
        .width(Length::FillPortion(portion))
        .height(Length::Fill)
        .clip(true)
        .style(move |_| container::Style {
            background: highlight.then(|| style::tint(t.bg, t.warning(), 0.10).into()),
            ..container::Style::default()
        })
        .into()
}

fn divider(t: &Tokens) -> Element<'_, Message> {
    container(space())
        .width(1)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t.border.into()),
            ..container::Style::default()
        })
        .into()
}

impl State {
    pub fn view<'a>(
        &'a self,
        t: &'a Tokens,
        files: &'a [PathBuf],
        drafts: &'a HashMap<PathBuf, Self>,
        completed: &'a [PathBuf],
        head: Option<&'a str>,
    ) -> Element<'a, Message> {
        let active = self.active;
        let differences = self.layout.differences.get(active).map_or(0, Vec::len);
        let block_count = self.resolutions.len();
        let another = files.len() > 1 || files.first().is_some_and(|file| *file != self.path);
        let unresolved = self.unresolved().unwrap_or(0);
        let first = self.difference == Some(0);
        let last = self
            .difference
            .is_some_and(|index| index + 1 >= differences);
        let prev = (differences > 0 && !first).then_some(Message::Difference(true));
        let next = (differences > 0 && !last).then_some(Message::Difference(false));
        let next_unresolved = ((another
            || unresolved
                > usize::from(self.resolutions.get(active).is_some_and(Option::is_none)))
            && self.edit.is_none())
        .then_some(Message::NextUnresolved);
        let have_base = self.file.as_ref().is_some_and(|file| {
            file.original.sources[0].is_some()
                || file.block(active).is_some_and(|block| block.base.is_some())
        });
        let header = row![
            action(
                t,
                if self.show_files {
                    "hide files"
                } else {
                    "files"
                },
                Some(Message::ToggleFiles)
            ),
            mono(t, widgets::format::repo_path(&self.path))
                .wrapping(Wrapping::None)
                .width(Length::Fill),
            dim(
                t,
                format!(
                    "{} · {}",
                    widgets::format::count(files.len() as u64, "file"),
                    widgets::format::count(unresolved as u64, "unresolved block")
                )
            ),
            widgets::icon_button(t, "✕", t.dim, "close resolver", Message::Close),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        let previous_block = active
            .checked_sub(1)
            .filter(|_| self.edit.is_none())
            .map(Message::SelectBlock);
        let next_block = (active + 1 < block_count && self.edit.is_none())
            .then_some(Message::SelectBlock(active + 1));
        let navigation = row![
            action(t, "‹ block", previous_block),
            dim(
                t,
                if block_count == 0 {
                    String::new()
                } else {
                    format!("{}/{}", active + 1, block_count)
                }
            ),
            action(t, "block ›", next_block),
            action(t, "‹ difference", prev),
            action(t, "difference ›", next),
            action(t, "next unresolved", next_unresolved.clone()),
            space().width(Length::Fill),
            action(
                t,
                if self.expanded {
                    "fold context"
                } else {
                    "full context"
                },
                self.file.as_ref().map(|_| Message::ToggleContext)
            ),
            action(
                t,
                if self.show_base { "hide base" } else { "base" },
                have_base.then_some(Message::ToggleBase)
            ),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);

        let content = if self.loading && self.file.is_none() {
            widgets::empty_hint(t, "reading conflict…")
        } else if let Some(file) = self.file.as_ref().filter(|file| !file.blocks.is_empty()) {
            responsive(move |size| {
                let narrow = size.width < t.cols(105);
                let columns = self.column_headers(t, narrow, head);
                let rows = virtual_list::virtual_list(
                    LIST,
                    self.layout.rows.len(),
                    row_height(t),
                    0.0,
                    self.scroll,
                    move |i| self.code_row(t, i, narrow),
                    Message::Scrolled,
                );
                let rows = if let Some(editor) = &self.edit {
                    let edit = column![
                        dim(t, "Edit the current block"),
                        text_editor(editor)
                            .id(EDITOR)
                            .font(crate::platform::mono_font())
                            .size(t.font_size)
                            .height(Length::Fill)
                            .wrapping(Wrapping::None)
                            .on_action(Message::EditAction)
                            .style(style::text_area(t)),
                        row![
                            action(t, "use edited block", Some(Message::AcceptEdit)),
                            action(t, "cancel", Some(Message::CancelEdit)),
                        ]
                        .spacing(4),
                    ]
                    .spacing(3);
                    let edit = container(edit)
                        .width(Length::FillPortion(3))
                        .height(Length::Fill)
                        .padding(4)
                        .style(move |_| container::Style {
                            background: Some(t.bg.into()),
                            ..container::Style::default()
                        });
                    stack![
                        rows,
                        row![
                            space().width(Length::FillPortion(self.left(narrow))),
                            edit,
                            space().width(Length::FillPortion(self.right(narrow))),
                        ]
                        .height(Length::Fill)
                    ]
                    .into()
                } else {
                    rows
                };
                let mut content = column![columns, rows].spacing(4);
                if self.show_base && have_base {
                    let section = file
                        .blocks
                        .get(active)
                        .filter(|block| !block.base.is_empty());
                    let lines = section.map_or(file.base.as_slice(), |block| &block.base);
                    let body = virtual_list::virtual_list(
                        BASE,
                        lines.len(),
                        row_height(t),
                        0.0,
                        self.base_scroll,
                        |i| mono(t, lines[i].as_str()).into(),
                        Message::BaseScrolled,
                    );
                    content = content.push(column![
                        dim(
                            t,
                            if section.is_some() {
                                "BASE · current block"
                            } else {
                                "BASE · full file"
                            }
                        ),
                        container(body).height(Length::Fixed(row_height(t) * 6.0)),
                    ]);
                }
                content.height(Length::Fill).into()
            })
            .into()
        } else {
            widgets::empty_hint(
                t,
                "no editable conflict blocks · choose a whole version or stage an externally resolved file",
            )
        };
        let body: Element<'_, Message> = if self.show_files {
            let list = self.file_list(t, files, drafts, completed);
            row![list, container(content).width(Length::Fill)]
                .spacing(8)
                .height(Length::Fill)
                .into()
        } else {
            container(content).height(Length::Fill).into()
        };
        let note = if let Some(error) = &self.error {
            error.clone()
        } else if self.resolved_on_disk {
            "✓ file resolved".into()
        } else if self.saving {
            "saving and staging…".into()
        } else if self.changed {
            "file changed on disk · draft kept".into()
        } else if let Some(error) = &self.refresh_error {
            error.clone()
        } else if self.edit.is_some() {
            "confirm your edit for this block".into()
        } else if unresolved > 0 {
            format!(
                "{} left",
                widgets::format::count(unresolved as u64, "unresolved block")
            )
        } else if block_count > 0 {
            "all conflicts chosen · save and stage this file".into()
        } else {
            "resolve the file using the available file actions".into()
        };
        let file = self.file.as_ref();
        let take = |side, i| {
            (!self.resolved_on_disk
                && !self.saving
                && file.is_some_and(|f| f.original.present[i])
                && self.edit.is_none())
            .then_some(Message::TakeFile(side))
        };
        let absent = file.and_then(|f| match (f.original.present[1], f.original.present[2]) {
            (false, true) => Some(Side::Ours),
            (true, false) => Some(Side::Theirs),
            _ => None,
        });
        let mut footer = row![
            dim(t, note).width(Length::Fill),
            action(
                t,
                "save & stage",
                self.can_apply().then_some(Message::Apply)
            ),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);
        if self.changed {
            footer = footer.push(action(
                t,
                "discard draft & reload",
                (!self.saving).then_some(Message::Reload),
            ));
        }
        if !self.resolved_on_disk && block_count > 0 {
            footer = footer.push(action(
                t,
                if self.file_actions {
                    "hide file actions"
                } else {
                    "file actions"
                },
                Some(Message::ToggleFileActions),
            ));
        }
        let mut body = column![header, navigation, body, footer]
            .spacing(4)
            .height(Length::Fill);
        if !self.resolved_on_disk && (self.file_actions || block_count == 0) {
            let mut actions = row![].spacing(4);
            for (label, message) in [
                ("whole file: ours", take(Side::Ours, 1)),
                ("whole file: theirs", take(Side::Theirs, 2)),
                (
                    "keep deletion",
                    absent.filter(|_| !self.saving).map(Message::TakeFile),
                ),
                (
                    "mark resolved",
                    (self.can_mark_resolved()
                        && !self.saving
                        && !self.changed
                        && !self.has_draft())
                    .then_some(Message::MarkResolved),
                ),
            ] {
                if message.is_some() {
                    actions = actions.push(action(t, label, message));
                }
            }
            body = body.push(actions);
        }

        widgets::accent_panel(t, "RESOLVE CONFLICT".into(), t.warning(), body)
    }

    fn column_headers<'a>(
        &'a self,
        t: &'a Tokens,
        narrow: bool,
        head: Option<&str>,
    ) -> Element<'a, Message> {
        let block = self.file.as_ref().and_then(|file| file.block(self.active));
        let can_choose =
            self.edit.is_none() && !self.changed && !self.saving && !self.resolved_on_disk;
        let side = |right| {
            let (name, version, choice) = if right {
                (
                    "Theirs",
                    block.map(|b| b.theirs_label.as_str()),
                    Choice::Theirs,
                )
            } else {
                ("Ours", block.map(|b| b.ours_label.as_str()), Choice::Ours)
            };
            let version = version.filter(|s| !s.is_empty() && *s != "HEAD");
            let label = match version.or(if right { None } else { head }) {
                Some(version) => format!("{version} · {name}"),
                None => name.into(),
            };
            let already = matches!(self.resolutions.get(self.active), Some(Some(Resolution::Side(c))) if *c == choice);
            let pick = if already {
                "✓ in result"
            } else if right {
                "← use block"
            } else {
                "use block →"
            };
            column![
                mono(t, label).wrapping(Wrapping::None),
                action(
                    t,
                    pick,
                    (can_choose && !already).then_some(Message::Choose(self.active, choice))
                ),
            ]
            .spacing(3)
            .width(Length::FillPortion(2))
        };
        let center_tools = row![
            action(t, "edit block", can_choose.then_some(Message::Edit)),
            action(
                t,
                "undo block",
                (self
                    .history
                    .get(self.active)
                    .is_some_and(|items| !items.is_empty())
                    && self.edit.is_none()
                    && !self.saving)
                    .then_some(Message::Undo)
            ),
            action(
                t,
                "options",
                can_choose.then_some(Message::ToggleBlockActions)
            ),
        ]
        .spacing(2);
        let mut result = column![mono(t, "Result"), center_tools]
            .spacing(3)
            .width(Length::FillPortion(3));
        if self.block_actions && can_choose {
            let mut options = row![
                action(
                    t,
                    "ours + theirs",
                    Some(Message::Choose(self.active, Choice::Both))
                ),
                action(
                    t,
                    "theirs + ours",
                    Some(Message::Choose(self.active, Choice::BothTheirsFirst))
                ),
            ]
            .spacing(2);
            if block.is_some_and(|block| block.base.is_some()) {
                options = options.push(action(
                    t,
                    "use base",
                    Some(Message::Choose(self.active, Choice::Base)),
                ));
            }
            result = result.push(options);
        }
        if narrow {
            result = result.push(action(
                t,
                match self.narrow_side {
                    Side::Ours => "show theirs",
                    Side::Theirs => "show ours",
                },
                Some(Message::SwitchSource),
            ));
        }
        let mut header = row![].spacing(0);
        if self.left(narrow) > 0 {
            header = header.push(side(false));
        }
        header = header.push(result);
        if self.right(narrow) > 0 {
            header = header.push(side(true));
        }
        header.into()
    }

    fn left(&self, narrow: bool) -> u16 {
        if narrow && self.narrow_side == Side::Theirs {
            0
        } else {
            2
        }
    }

    fn right(&self, narrow: bool) -> u16 {
        if narrow && self.narrow_side == Side::Ours {
            0
        } else {
            2
        }
    }

    fn code_row<'a>(&'a self, t: &'a Tokens, n: usize, narrow: bool) -> Element<'a, Message> {
        let Some(file) = &self.file else {
            return space().into();
        };
        let render = |left: &str, center: &str, right: &str, changed| {
            let mut line = row![].height(Length::Fill);
            if self.left(narrow) > 0 {
                line = line.push(cell(t, left, 2, changed)).push(divider(t));
            }
            line = line.push(cell(t, center, 3, changed));
            if self.right(narrow) > 0 {
                line = line.push(divider(t)).push(cell(t, right, 2, changed));
            }
            line.into()
        };
        match self.layout.rows[n] {
            Row::Common { segment, line } => {
                let source = |i: usize| {
                    file.common_sources[i][segment]
                        .get(line)
                        .and_then(Option::as_deref)
                        .unwrap_or_default()
                };
                render(source(0), &file.common[segment][line], source(1), false)
            }
            Row::Heading(block) => {
                let resolved = self.resolutions[block].is_some();
                let line = file.blocks[block].line;
                widgets::text_button(
                    t,
                    &format!(
                        "{}   block {} · line {line}",
                        if resolved {
                            "✓ resolved"
                        } else {
                            "○ unresolved"
                        },
                        block + 1
                    ),
                    None,
                    (block != self.active && self.edit.is_none())
                        .then_some(Message::SelectBlock(block)),
                )
            }
            Row::Code { block, index } => {
                let data = &file.blocks[block];
                let row = &self.aligned[block][index];
                let sides = row.source.map(|pair| &data.pairs[pair]);
                let get = |lines: &[String], index: Option<usize>| {
                    index
                        .and_then(|i| lines.get(i))
                        .map_or_else(String::new, Clone::clone)
                };
                let center = if index == 0 && self.resolutions[block].is_none() {
                    "○ Choose a side or edit".into()
                } else {
                    get(&self.result[block], row.result)
                };
                render(
                    &get(&data.sides[0], sides.and_then(|pair| pair.ours)),
                    &center,
                    &get(&data.sides[1], sides.and_then(|pair| pair.theirs)),
                    row.changed || self.focused_row == Some(n),
                )
            }
            Row::Fold(count) => render("", &format!("… {count} lines folded"), "", false),
        }
    }

    fn file_list<'a>(
        &'a self,
        t: &'a Tokens,
        files: &'a [PathBuf],
        drafts: &'a HashMap<PathBuf, Self>,
        completed: &'a [PathBuf],
    ) -> Element<'a, Message> {
        let current = files.binary_search(&self.path).ok();
        let start = current.map_or(files.len(), |index| index + 1);
        let block_count = current.map_or(0, |_| self.resolutions.len());
        let sections = virtual_list::virtual_list(
            FILES,
            files.len() + block_count + completed.len(),
            row_height(t),
            0.0,
            self.file_scroll,
            move |index| {
                if (start..start + block_count).contains(&index) {
                    let i = index - start;
                    let Some(block) = self.file.as_ref().and_then(|file| file.blocks.get(i)) else {
                        return space().into();
                    };
                    let mark = if self.resolutions[i].is_some() {
                        "✓"
                    } else {
                        "○"
                    };
                    return action(
                        t,
                        &format!("  {mark} line {}", block.line),
                        (self.edit.is_none() && self.active != i)
                            .then_some(Message::SelectBlock(i)),
                    );
                }
                let file_index = if index < start {
                    index
                } else {
                    index - block_count
                };
                if let Some(path) = files.get(file_index) {
                    let selected = Some(file_index) == current;
                    let state = if selected {
                        Some(self)
                    } else {
                        drafts.get(path)
                    };
                    let count = state
                        .and_then(Self::unresolved)
                        .map_or_else(String::new, |n| format!(" · {n}"));
                    return action(
                        t,
                        &format!(
                            "{}{}{}",
                            if selected { "› " } else { "  " },
                            widgets::format::repo_path(path),
                            count
                        ),
                        (!selected && !self.saving).then(|| Message::OpenFile(path.clone())),
                    );
                }
                let path = &completed[file_index - files.len()];
                dim(t, format!("✓ {}", widgets::format::repo_path(path))).into()
            },
            Message::FilesScrolled,
        );
        container(column![dim(t, "CONFLICTED FILES"), sections].spacing(4))
            .width(Length::Fixed(t.cols(29)))
            .height(Length::Fill)
            .into()
    }
}
