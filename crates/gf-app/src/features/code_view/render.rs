//! Shared line rendering for the inline and side-by-side layouts.

use gf_core::model::LineKind;
use iced::widget::text::Wrapping;
use iced::widget::{column, container, mouse_area, rich_text, row, span};
use iced::{Background, Element, Length, Padding};

use super::{
    COMPARE_GUTTER, Code, LIST_ID, Message, Pane, PreviewRow, State, comparison, row_height,
    scroll_marks,
};
use crate::highlight::Spans;
use crate::theme::{Tokens, style};
use crate::widgets::selectable_line::{Geometry, selectable_line};
use crate::widgets::virtual_list::{virtual_gutter, virtual_list};
use crate::widgets::{self, colored, dim, format};

impl State {
    pub(super) fn lines<'a>(&'a self, t: &'a Tokens, code: &'a Code) -> Element<'a, Message> {
        let digits = code.max_line_number.to_string().len().max(3);
        let list = virtual_list(
            LIST_ID,
            code.preview_rows
                .as_ref()
                .map_or(code.lines.len(), Vec::len),
            row_height(t),
            t.cols(digits + code.widest + 5),
            self.scroll,
            move |row| {
                let (index, kind) = match code.preview_rows.as_ref().map(|rows| &rows[row]) {
                    None => (row, LineKind::Context),
                    Some(PreviewRow::File(index, kind)) => (*index, *kind),
                    Some(PreviewRow::Removed { number, text }) => {
                        let hits = self.find.row_hits(row, Pane::After);
                        let text = code_text(t, text, None, hits);
                        return row_content(
                            t,
                            Some((digits, *number as usize)),
                            LineKind::Removed,
                            text,
                        )
                        .0
                        .style(move |_| container::Style {
                            background: style::diff_line_color(t, LineKind::Removed)
                                .map(Background::Color),
                            ..container::Style::default()
                        })
                        .into();
                    }
                };
                let hits = self.find.row_hits(row, Pane::After);
                self.source_line(t, code, Pane::After, index, Some(digits), kind, hits)
            },
            Message::Scrolled,
        );
        let list = if code.scroll_marks.is_empty() {
            list
        } else {
            row![list, scroll_marks::view(t, &code.scroll_marks)].into()
        };
        self.selection_area(list)
    }

    #[allow(clippy::too_many_arguments)] // The display row differs from the source line in diffs.
    fn source_line<'a>(
        &'a self,
        t: &'a Tokens,
        code: &'a Code,
        pane: Pane,
        index: usize,
        digits: Option<usize>,
        kind: LineKind,
        hits: impl Iterator<Item = (std::ops::Range<usize>, bool)>,
    ) -> Element<'a, Message> {
        let raw = code.line(index);
        let text = code_text(
            t,
            raw,
            code.spans.as_ref().and_then(|spans| spans.get(index)),
            hits,
        );
        let (line, marker) = row_content(t, digits.map(|d| (d, index + 1)), kind, text);
        let code_x = t.cols(digits.unwrap_or(0) + 3);
        let selection = if self.active_pane == pane {
            code.selection_on(index, self.selection())
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };
        selectable_line(
            line,
            raw,
            Geometry {
                marker_x: code_x - t.char_width(),
                code_x,
                cell: t.char_width(),
                size: t.font_size,
            },
            move |pointer| Message::Pressed(pane, index, pointer),
        )
        .background(style::diff_line_color(t, kind))
        .selected(selection, t.diff_selection, marker)
        .on_drag(self.dragging && self.active_pane == pane, move |pointer| {
            Message::Dragged(pane, index, pointer)
        })
        .into()
    }

    pub(super) fn comparison_view<'a>(
        &'a self,
        t: &'a Tokens,
        comparison: &'a comparison::Comparison,
        labels: [&'static str; 2],
    ) -> Element<'a, Message> {
        let caption_height = row_height(t);
        let has_marks = !comparison.marks.is_empty();
        let caption = |side, label: &'static str| {
            let label = if self.pane_code(side).is_none() {
                format!("{label} · no file")
            } else {
                label.to_owned()
            };
            container(dim(t, label))
                .padding(Padding::from([0, 8]))
                .center_y(caption_height)
                .width(Length::FillPortion(1))
        };
        let pane = |side: Pane| {
            let code = self.pane_code(side);
            let id = side.list_id();
            let list = virtual_list(
                id,
                comparison.rows.len(),
                row_height(t),
                t.cols(code.map_or(0, |c| c.widest) + 5),
                self.scroll,
                move |row| {
                    let (index, kind) = comparison_line(comparison.rows[row], side);
                    match (code, index) {
                        (Some(source), Some(index)) => {
                            let hits = self.find.row_hits(row, side);
                            self.source_line(t, source, side, index, None, kind, hits)
                        }
                        _ => iced::widget::space().into(),
                    }
                },
                move |pos| Message::CompareScrolled(id, pos),
            );
            let list = if side == Pane::After && has_marks {
                row![list, scroll_marks::view(t, &comparison.marks)].into()
            } else {
                list
            };
            container(list).width(Length::FillPortion(1))
        };
        let longest = [Pane::Before, Pane::After]
            .into_iter()
            .filter_map(|side| self.pane_code(side))
            .map(|code| code.lines.len())
            .max()
            .unwrap_or(0);
        let digits = longest.to_string().len().max(3);
        let number_width = t.cols(digits + 2);
        let gutter_width = 2.0 * number_width + 1.0;
        let gutter = virtual_gutter(
            COMPARE_GUTTER,
            comparison.rows.len(),
            row_height(t),
            gutter_width,
            self.scroll,
            |index| {
                let aligned = comparison.rows[index];
                let cell = |side| {
                    let (index, kind) = comparison_line(aligned, side);
                    gutter_number(
                        t,
                        digits,
                        number_width,
                        side,
                        index,
                        kind,
                        self.dragging && self.active_pane == side,
                    )
                };
                row![
                    cell(Pane::Before),
                    container(iced::widget::space().width(1).height(Length::Fill))
                        .style(move |_| container::Style::default().background(t.border)),
                    cell(Pane::After),
                ]
                .into()
            },
            |pos| Message::CompareScrolled(COMPARE_GUTTER, pos),
        );
        let gutter =
            container(gutter).style(move |_| container::Style::default().background(t.surface));
        let right_caption = caption(Pane::After, labels[1]);
        let right_caption: Element<'a, Message> = if has_marks {
            row![right_caption, scroll_marks::cap(t, caption_height)]
                .width(Length::FillPortion(1))
                .into()
        } else {
            right_caption.into()
        };
        column![
            row![
                caption(Pane::Before, labels[0]),
                iced::widget::space().width(gutter_width),
                right_caption
            ],
            self.selection_area(row![pane(Pane::Before), gutter, pane(Pane::After)].into()),
        ]
        .into()
    }

    fn selection_area<'a>(&self, list: Element<'a, Message>) -> Element<'a, Message> {
        if self.dragging {
            mouse_area(list)
                .on_release(Message::DragEnd)
                .on_exit(Message::DragEnd)
                .into()
        } else {
            mouse_area(list).into()
        }
    }
}

/// Split syntax spans at match boundaries while retaining their colors outside matches.
fn code_text<'a>(
    t: &'a Tokens,
    raw: &'a str,
    syntax: Option<&'a Spans>,
    hits: impl Iterator<Item = (std::ops::Range<usize>, bool)>,
) -> Element<'a, Message> {
    let mut hits = hits.peekable();
    if hits.peek().is_none() {
        return match syntax {
            Some(spans) => widgets::highlighted_line(t, spans),
            None => widgets::mono(t, format::code_line(raw))
                .wrapping(Wrapping::None)
                .into(),
        };
    }
    let colors = syntax
        .into_iter()
        .flatten()
        .map(|(tone, text)| (text.len(), t.tone(*tone)))
        .chain(std::iter::once((raw.len(), t.fg)));
    let mut spans = Vec::new();
    let mut at = 0;
    for (length, color) in colors {
        let end = (at + length).min(raw.len());
        while at < end {
            let (next, is_match) = match hits.peek() {
                Some((range, active)) if range.start <= at => (range.end.min(end), Some(*active)),
                Some((range, _)) => (range.start.min(end), None),
                None => (end, None),
            };
            let text = if next == raw.len() {
                format::code_line(&raw[at..next])
            } else {
                format::code_text(&raw[at..next])
            };
            let piece = span::<(), iced::Font>(text).color(color);
            spans.push(if let Some(active) = is_match {
                let (background, foreground) = t.find_match(active);
                piece.background(background).color(foreground)
            } else {
                piece
            });
            at = next;
            if hits.peek().is_some_and(|(range, _)| range.end == at) {
                hits.next();
            }
        }
        if at == raw.len() {
            break;
        }
    }
    rich_text(spans)
        .size(t.font_size)
        .wrapping(Wrapping::None)
        .into()
}

fn row_content<'a>(
    t: &Tokens,
    number: Option<(usize, usize)>,
    kind: LineKind,
    text: Element<'a, Message>,
) -> (iced::widget::Container<'a, Message>, char) {
    let (marker, color) = match kind {
        LineKind::Context => (' ', t.dim),
        LineKind::Added => ('+', t.added()),
        LineKind::Removed => ('−', t.removed()),
    };
    let gutter: Element<'a, Message> = match number {
        Some((digits, number)) => container(dim(t, format!("{number:>digits$}")))
            .width(Length::Fixed(t.cols(digits + 1)))
            .into(),
        None => iced::widget::space()
            .width(Length::Fixed(t.char_width()))
            .into(),
    };
    let row = row![
        gutter,
        iced::widget::space().width(Length::Fixed(t.char_width())),
        container(colored(t, marker.to_string(), color)).width(Length::Fixed(t.char_width())),
        text,
    ]
    .align_y(iced::Alignment::Center);
    (
        container(row).width(Length::Fill).height(Length::Fill),
        marker,
    )
}

fn comparison_line(row: comparison::Row, side: Pane) -> (Option<usize>, LineKind) {
    let (index, change) = match side {
        Pane::Before => (row.before, LineKind::Removed),
        Pane::After => (row.after, LineKind::Added),
    };
    (
        index,
        if row.changed {
            change
        } else {
            LineKind::Context
        },
    )
}

fn gutter_number(
    t: &Tokens,
    digits: usize,
    width: f32,
    side: Pane,
    index: Option<usize>,
    kind: LineKind,
    dragging: bool,
) -> Element<'_, Message> {
    let Some(index) = index else {
        return iced::widget::space()
            .width(width)
            .height(Length::Fill)
            .into();
    };
    let number = index + 1;
    let number = container(dim(t, format!("{number:>digits$}")))
        .center_x(width)
        .center_y(Length::Fill);
    selectable_line(
        number,
        "",
        Geometry {
            marker_x: width,
            code_x: width + t.char_width(),
            cell: t.char_width(),
            size: t.font_size,
        },
        move |pointer| Message::Pressed(side, index, pointer),
    )
    .background(style::diff_line_color(t, kind))
    .on_drag(dragging, move |pointer| {
        Message::Dragged(side, index, pointer)
    })
    .into()
}
