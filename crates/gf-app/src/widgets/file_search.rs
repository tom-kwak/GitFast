//! Search state and match rendering shared by the file and blame views.

use std::ops::Range;
use std::sync::Arc;

use iced::advanced::graphics::text::Paragraph;
use iced::advanced::text::{self, Paragraph as _};
use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};
use iced::advanced::widget::{Id, Operation};
use iced::widget::text::Wrapping;
use iced::widget::{operation, rich_text, row, span, text_input};
use iced::{Element, Font, Length, Pixels, Rectangle, Size, Task, Vector, alignment};
use regex::RegexBuilder;

use crate::highlight::Spans;
use crate::theme::{Tokens, style};

use super::{self as widgets, dim, format};

#[derive(Debug, Clone)]
pub struct Search<P> {
    open: bool,
    query: String,
    generation: u64,
    pending: bool,
    hits: Arc<[Hit<P>]>,
    active: usize,
}

/// A match in display order, with a byte range in the original line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit<P> {
    pub row: u32,
    pub pane: P,
    start: u32,
    end: u32,
}

impl<P> Hit<P> {
    pub fn range(&self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

impl<P> Default for Search<P> {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            generation: 0,
            pending: false,
            hits: Arc::default(),
            active: 0,
        }
    }
}

impl<P: PartialEq> Search<P> {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) -> bool {
        !std::mem::replace(&mut self.open, true)
    }

    pub fn close(&mut self) {
        self.open = false;
        self.clear_results();
    }

    pub fn set_query(&mut self, query: String) {
        self.query = query;
    }

    pub fn restart(&mut self) -> Option<(u64, String)> {
        self.clear_results();
        if !self.open || self.query.is_empty() {
            return None;
        }
        self.pending = true;
        Some((self.generation, self.query.clone()))
    }

    pub fn accept(&mut self, generation: u64, hits: Arc<[Hit<P>]>) -> bool {
        if self.generation != generation || !self.pending {
            return false;
        }
        self.pending = false;
        self.hits = hits;
        !self.hits.is_empty()
    }

    pub fn move_match(&mut self, back: bool) -> bool {
        let len = self.hits.len();
        if len == 0 {
            return false;
        }
        self.active = if back {
            self.active.checked_sub(1).unwrap_or(len - 1)
        } else {
            (self.active + 1) % len
        };
        true
    }

    pub fn active_match(&self) -> Option<&Hit<P>> {
        self.hits.get(self.active)
    }

    pub fn row_hits(&self, row: usize, pane: P) -> impl Iterator<Item = (Range<usize>, bool)> + '_ {
        let start = self.hits.partition_point(|hit| (hit.row as usize) < row);
        self.hits[start..]
            .iter()
            .take_while(move |hit| hit.row as usize == row)
            .enumerate()
            .filter(move |(_, hit)| hit.pane == pane)
            .map(move |(i, hit)| (hit.range(), start + i == self.active))
    }

    fn clear_results(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
        self.hits = Arc::default();
        self.active = 0;
    }

    pub fn view<'a, M: Clone + 'a>(
        &'a self,
        t: &'a Tokens,
        field: &'static str,
        query: impl Fn(String) -> M + 'a,
        move_match: impl Fn(bool) -> M,
        close: M,
    ) -> Element<'a, M> {
        let count = match (self.pending, self.hits.len()) {
            (true, _) => "Searching…".into(),
            (false, 0) if self.query.is_empty() => String::new(),
            (false, 0) => "0 of 0".into(),
            (false, len) => format!("{} of {len}", self.active + 1),
        };
        let can_move = self.hits.len() > 1;
        row![
            text_input("Find in file…", &self.query)
                .id(field)
                .on_input(query)
                .size(t.font_size)
                .padding(4)
                .width(Length::Fill)
                .style(style::field_input(t)),
            dim(t, count),
            widgets::icon_button_maybe(
                t,
                "↑",
                t.accent(),
                "Previous match · ⇧Enter / ⌘⇧G",
                can_move.then(|| move_match(true)),
            ),
            widgets::icon_button_maybe(
                t,
                "↓",
                t.accent(),
                "Next match · Enter / ⌘G",
                can_move.then(|| move_match(false)),
            ),
            widgets::icon_button(t, "✕", t.dim, "Close search · Esc", close),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .into()
    }
}

/// Search displayed lines in order, preserving byte offsets in the source text.
pub fn matches<'a, P: Copy>(
    query: &str,
    rows: impl IntoIterator<Item = (usize, P, &'a str)>,
) -> Arc<[Hit<P>]> {
    let Ok(pattern) = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
    else {
        return Arc::default();
    };
    let mut hits = Vec::new();
    for (row, pane, text) in rows {
        let Ok(row) = u32::try_from(row) else { break };
        hits.extend(pattern.find_iter(text).map_while(|m| {
            Some(Hit {
                row,
                pane,
                start: u32::try_from(m.start()).ok()?,
                end: u32::try_from(m.end()).ok()?,
            })
        }));
    }
    hits.into()
}

pub fn focus<M: Send + 'static>(field: &'static str) -> Task<M> {
    operation::focus(field).chain(operation::select_all(field))
}

/// Width in columns with the same four spaces used to display a tab.
pub fn line_width(line: &str) -> usize {
    widgets::virtual_list::columns(line) + 3 * line.bytes().filter(|&byte| byte == b'\t').count()
}

pub fn reveal_match<M: Send + 'static, P>(
    hit: &Hit<P>,
    raw: &str,
    t: &Tokens,
    list: &'static str,
    code_x: f32,
    row_height: f32,
) -> Task<M> {
    #[allow(clippy::cast_precision_loss)] // The row index becomes a pixel coordinate.
    let y = hit.row as f32 * row_height;
    let range = hit.range();
    let x = match_bounds(t, raw, range.clone()).unwrap_or_else(|| {
        let x = |byte| t.cols(line_width(&raw[..byte]));
        x(range.start)..x(range.end)
    });
    iced::advanced::widget::operate(Reveal {
        target: Id::from(list),
        x: code_x + x.start..code_x + x.end,
        y: y..y + row_height,
    })
}

// Use the same rich-text shaping as the rendered matches. A fractional
// error per column can otherwise scroll a distant match off the screen.
fn match_bounds(t: &Tokens, raw: &str, range: Range<usize>) -> Option<Range<f32>> {
    let spans = [
        span::<(), Font>(format::code_text(&raw[..range.start])),
        span(if range.end == raw.len() {
            format::code_line(&raw[range.clone()])
        } else {
            format::code_text(&raw[range.clone()])
        }),
        span(format::code_line(&raw[range.end..])),
    ];
    let paragraph = Paragraph::with_spans(text::Text {
        content: &spans,
        bounds: Size::INFINITE,
        size: Pixels(t.font_size),
        line_height: text::LineHeight::default(),
        font: crate::platform::mono_font(),
        align_x: text::Alignment::Default,
        align_y: alignment::Vertical::Top,
        shaping: text::Shaping::Advanced,
        wrapping: Wrapping::None,
    });
    let mut bounds = paragraph.span_bounds(1).into_iter();
    let first = bounds.next()?;
    Some(
        bounds.fold(first.x..first.x + first.width, |range, bounds| {
            range.start.min(bounds.x)..range.end.max(bounds.x + bounds.width)
        }),
    )
}

/// Split syntax spans at match boundaries while retaining their colors outside matches.
pub fn code_text<'a, M: 'a>(
    t: &'a Tokens,
    raw: &'a str,
    syntax: Option<&'a Spans>,
    hits: impl Iterator<Item = (std::ops::Range<usize>, bool)>,
) -> Element<'a, M> {
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

struct Reveal {
    target: Id,
    x: Range<f32>,
    y: Range<f32>,
}

impl<M> Operation<M> for Reveal {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<M>)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content: Rectangle,
        translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        if id != Some(&self.target) {
            return;
        }
        let margin = crate::widgets::virtual_list::SCROLLBAR;
        let width = bounds.width
            - if content.height > bounds.height {
                margin
            } else {
                0.0
            };
        let height = bounds.height
            - if content.width > bounds.width {
                margin
            } else {
                0.0
            };
        let x = reveal_axis(&self.x, translation.x, width);
        let y = reveal_axis(&self.y, translation.y, height)
            .map(|_| ((self.y.start + self.y.end - height) / 2.0).max(0.0));
        if x.is_some() || y.is_some() {
            state.scroll_to(AbsoluteOffset { x, y });
        }
    }
}

fn reveal_axis(hit: &Range<f32>, offset: f32, size: f32) -> Option<f32> {
    if hit.start < offset {
        Some(hit.start)
    } else if hit.end > offset + size {
        Some((hit.end - size).min(hit.start))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveals_offscreen_matches_without_moving_visible_matches() {
        assert_eq!(reveal_axis(&(130.0..140.0), 100.0, 50.0), None);
        assert_eq!(reveal_axis(&(80.0..85.0), 100.0, 50.0), Some(80.0));
        assert_eq!(reveal_axis(&(160.0..170.0), 100.0, 50.0), Some(120.0));
        assert_eq!(reveal_axis(&(110.0..200.0), 100.0, 50.0), Some(110.0));
    }
}
