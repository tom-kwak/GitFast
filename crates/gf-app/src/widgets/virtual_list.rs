//! Virtualized list with horizontal scrolling.
//!
//! - Only rows inside the viewport (plus a margin) are built, so lists of
//!   thousands of commits or diff lines cost the same as a screenful. iced
//!   rebuilds the whole view on every message; without this a single click
//!   on a long history re-created thousands of widgets.
//! - Rows are as wide as the widest content (measured in monospace columns
//!   by the caller) or the viewport, whichever is larger, and the list
//!   scrolls sideways when content is wider — nothing wraps or gets cut.

use std::ops::Range;

use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::{Column, container, responsive, scrollable, space};
use iced::{Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector};

/// Rows built beyond each edge of the viewport, so fast scrolling does not
/// show blank space for a frame.
const OVERSCAN: usize = 10;
/// Assumed viewport height before the first scroll event reports the real one.
const INITIAL_HEIGHT: f32 = 1400.0;
/// Room for the vertical scrollbar.
pub const SCROLLBAR: f32 = 8.0;

/// Scroll position reported by the list (kept in the feature's state).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScrollPos {
    pub offset: f32,
    pub height: f32,
    /// Height of everything in the list (0 until the first scroll).
    pub content: f32,
}

impl ScrollPos {
    fn from_viewport(v: scrollable::Viewport) -> Self {
        Self {
            offset: v.absolute_offset().y,
            height: v.bounds().height,
            content: v.content_bounds().height,
        }
    }

    /// Whether the viewport is within `margin` pixels of the list's end.
    pub fn near_end(&self, margin: f32) -> bool {
        self.content > 0.0 && self.offset + self.height >= self.content - margin
    }

    /// Indices of the rows to build.
    pub fn visible(&self, total: usize, row_height: f32) -> Range<usize> {
        if total == 0 || row_height <= 0.0 {
            return 0..0;
        }
        let height = if self.height > 0.0 {
            self.height
        } else {
            INITIAL_HEIGHT
        };
        // Truncation is intended: these are row indices.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let first = (self.offset.max(0.0) / row_height) as usize;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let count = (height / row_height).ceil() as usize;
        let start = first.saturating_sub(OVERSCAN).min(total);
        let end = (first + count + OVERSCAN).min(total);
        start..end.max(start)
    }
}

/// Builds the list. `render(i)` creates row `i`; `on_scroll` reports the
/// new position so the owner can store it and re-render. `content_width` is
/// the width the widest row needs.
#[allow(clippy::too_many_arguments)]
pub fn virtual_list<'a, M: 'a>(
    id: &'static str,
    total: usize,
    row_height: f32,
    content_width: f32,
    pos: ScrollPos,
    render: impl Fn(usize) -> Element<'a, M> + 'a,
    on_scroll: impl Fn(ScrollPos) -> M + Clone + 'a,
) -> Element<'a, M> {
    responsive(move |size| {
        let width = content_width.max(size.width - SCROLLBAR);
        let col = virtual_rows(total, row_height, width, pos, &render);
        let on_scroll = on_scroll.clone();
        scrollable(col)
            .id(id)
            .direction(scrollable::Direction::Both {
                vertical: scrollable::Scrollbar::new().width(6).scroller_width(6),
                horizontal: scrollable::Scrollbar::new().width(6).scroller_width(6),
            })
            .on_scroll(move |v| on_scroll(ScrollPos::from_viewport(v)))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    })
    .into()
}

/// A fixed-width gutter that scrolls vertically with its adjacent lists.
#[allow(clippy::too_many_arguments)]
pub fn virtual_gutter<'a, M: 'a>(
    id: &'static str,
    total: usize,
    row_height: f32,
    width: f32,
    pos: ScrollPos,
    render: impl Fn(usize) -> Element<'a, M>,
    on_scroll: impl Fn(ScrollPos) -> M + 'a,
) -> Element<'a, M> {
    scrollable(virtual_rows(total, row_height, width, pos, render))
        .id(id)
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::hidden(),
        ))
        .on_scroll(move |v| on_scroll(ScrollPos::from_viewport(v)))
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .into()
}

fn virtual_rows<'a, M: 'a>(
    total: usize,
    row_height: f32,
    width: f32,
    pos: ScrollPos,
    render: impl Fn(usize) -> Element<'a, M>,
) -> Element<'a, M> {
    let range = pos.visible(total, row_height);
    #[allow(clippy::cast_precision_loss)]
    let px = |rows: usize| rows as f32 * row_height;
    let mut children = Vec::with_capacity(range.len() + 2);
    children.push(space().height(Length::Fixed(px(range.start))).into());
    for i in range.clone() {
        children.push(
            container(render(i))
                .height(Length::Fixed(row_height))
                .width(Length::Fixed(width))
                .clip(true)
                .into(),
        );
    }
    children.push(space().height(Length::Fixed(px(total - range.end))).into());
    // Column::push skips zero-height spacers, but their slots are fixed.
    let column = Column::from_vec(children).width(Length::Fixed(width));
    Element::new(VisibleRows { range, column })
}

/// Keep each visible row's widget state (including text layout) attached
/// to its row index as the viewport shifts. The column holds a spacer at
/// each end, so actual rows always begin at child index one.
struct VisibleRows<'a, M> {
    range: Range<usize>,
    column: Column<'a, M>,
}

struct VisibleRange(Range<usize>);

fn reconcile_row_states(children: &mut Vec<Tree>, previous: &Range<usize>, next: &Range<usize>) {
    struct NewRow;

    let mut old = std::mem::take(children).into_iter();
    let top = old.next().expect("top spacer");
    let bottom = old.next_back().expect("bottom spacer");
    let mut old = old.skip(next.start.saturating_sub(previous.start));
    children.reserve(next.len() + 2);
    children.push(top);
    children.extend(next.clone().map(|index| {
        if previous.contains(&index) {
            old.next().expect("overlapping row")
        } else {
            Tree {
                tag: tree::Tag::of::<NewRow>(),
                ..Tree::empty()
            }
        }
    }));
    children.push(bottom);
}

impl<M> Widget<M, Theme, Renderer> for VisibleRows<'_, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<VisibleRange>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(VisibleRange(self.range.clone()))
    }

    fn children(&self) -> Vec<Tree> {
        self.column.children()
    }

    fn diff(&self, tree: &mut Tree) {
        let previous = &mut tree.state.downcast_mut::<VisibleRange>().0;
        if *previous != self.range {
            reconcile_row_states(&mut tree.children, previous, &self.range);
            *previous = self.range.clone();
        }
        self.column.diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.column.size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.column.layout(tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.column.operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        self.column.update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.column
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.column
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        self.column
            .overlay(tree, layout, renderer, viewport, translation)
    }
}

/// Display width of `s` in monospace columns: East Asian wide characters
/// (Hangul, CJK, full-width forms) take two columns.
pub fn columns(s: &str) -> usize {
    s.chars().map(char_columns).sum()
}

/// Display width of one character: see [`columns`].
pub fn char_columns(c: char) -> usize {
    let u = u32::from(c);
    let wide = matches!(u,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA960..=0xA97F | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x20000..=0x3FFFD);
    if wide { 2 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_range_follows_the_viewport() {
        let pos = ScrollPos {
            offset: 2000.0,
            height: 400.0,
            ..ScrollPos::default()
        };
        // Rows of 20px: first visible is 100, 20 rows fit.
        assert_eq!(pos.visible(10_000, 20.0), 90..130);
        assert_eq!(pos.visible(105, 20.0), 90..105, "clamped to the end");
        assert_eq!(ScrollPos::default().visible(0, 20.0), 0..0);
    }

    #[test]
    fn before_first_scroll_assumes_a_screenful() {
        let r = ScrollPos::default().visible(10_000, 20.0);
        assert_eq!(r.start, 0);
        assert!(r.end >= 70 && r.end < 200, "{r:?}");
    }

    #[test]
    fn preserves_row_state_when_the_visible_range_moves_or_resizes() {
        fn state(index: usize) -> Tree {
            Tree {
                state: tree::State::new(index),
                ..Tree::empty()
            }
        }

        let previous = 5..10;
        for next in [3..8, 7..12, 6..9, 4..11, 20..24, 0..0] {
            let mut children: Vec<_> = std::iter::once(state(0))
                .chain(previous.clone().map(state))
                .chain(std::iter::once(state(1)))
                .collect();
            reconcile_row_states(&mut children, &previous, &next);
            assert_eq!(children.len(), next.len() + 2);
            assert_eq!(*children[0].state.downcast_ref::<usize>(), 0);
            for (index, child) in next.zip(&children[1..]) {
                if previous.contains(&index) {
                    assert_eq!(*child.state.downcast_ref::<usize>(), index);
                } else {
                    assert!(matches!(child.state, tree::State::None));
                }
            }
            assert_eq!(
                *children
                    .last()
                    .expect("bottom spacer")
                    .state
                    .downcast_ref::<usize>(),
                1
            );
        }
    }

    #[test]
    fn counts_wide_characters_twice() {
        assert_eq!(columns("main"), 4);
        assert_eq!(columns("기능/로그인"), 11);
        assert_eq!(columns("feat: 한글"), 10);
    }
}
