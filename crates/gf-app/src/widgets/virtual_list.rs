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

use iced::widget::{Column, container, responsive, scrollable, space};
use iced::{Element, Length};

/// Rows built beyond each edge of the viewport, so fast scrolling does not
/// show blank space for a frame.
const OVERSCAN: usize = 10;
/// Assumed viewport height before the first scroll event reports the real one.
const INITIAL_HEIGHT: f32 = 1400.0;
/// Room for the vertical scrollbar.
const SCROLLBAR: f32 = 8.0;

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
        let range = pos.visible(total, row_height);
        let mut col = Column::new().width(Length::Fixed(width));
        #[allow(clippy::cast_precision_loss)]
        let px = |rows: usize| rows as f32 * row_height;
        if range.start > 0 {
            col = col.push(space().height(Length::Fixed(px(range.start))));
        }
        for i in range.clone() {
            col = col.push(
                container(render(i))
                    .height(Length::Fixed(row_height))
                    .width(Length::Fixed(width))
                    .clip(true),
            );
        }
        if range.end < total {
            col = col.push(space().height(Length::Fixed(px(total - range.end))));
        }
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

/// Display width of `s` in monospace columns: East Asian wide characters
/// (Hangul, CJK, full-width forms) take two columns.
pub fn columns(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let u = u32::from(c);
            let wide = matches!(u,
                0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
                | 0xA960..=0xA97F | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60
                | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x20000..=0x3FFFD);
            if wide { 2 } else { 1 }
        })
        .sum()
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
    fn counts_wide_characters_twice() {
        assert_eq!(columns("main"), 4);
        assert_eq!(columns("기능/로그인"), 11);
        assert_eq!(columns("feat: 한글"), 10);
    }
}
