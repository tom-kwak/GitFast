//! Commit graph row drawn as vector lines on a canvas.
//!
//! Each row draws its [`Edge`]s from the very top to the very bottom of the
//! row, so lines join seamlessly with the rows above and below. Lane
//! changes are smooth curves; lanes are only [`LANE_SPACING`] px apart, far
//! narrower than the two characters per lane of a text graph.
//!
//! The drawn geometry is cached per row widget ([`RowCache`]): the window
//! redraws on every message and hover, and tessellating each visible row's
//! curves every time is wasted work while the rows stay the same.

use gf_git::graph::{Anchor, GraphRow};
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme, mouse};
use std::cell::RefCell;

/// Horizontal distance between lanes.
pub const LANE_SPACING: f32 = 11.0;
/// Space left of the first lane.
const PAD: f32 = 7.0;
/// Lanes drawn at most; wider graphs are clipped with a fade marker.
pub const MAX_LANES: usize = 14;
/// How much of a cut line's length is drawn before its arrow head.
const ARROW_REACH: f32 = 0.85;
/// Length of an arrow head.
const ARROW_HEAD: f32 = 4.5;

/// Width needed to draw `lanes` lanes (capped at [`MAX_LANES`]).
pub fn width_for(lanes: usize) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let n = lanes.clamp(1, MAX_LANES) as f32;
    PAD * 2.0 + (n - 1.0) * LANE_SPACING
}

struct RowGraph<'a> {
    row: &'a GraphRow,
    colors: [Color; 6],
    /// Background behind the dot of a merge commit (drawn as a ring).
    background: Color,
}

impl RowGraph<'_> {
    fn color(&self, i: usize) -> Color {
        self.colors[i % self.colors.len()]
    }
}

fn x(lane: usize) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let lane = lane as f32;
    PAD + lane * LANE_SPACING
}

/// Geometry drawn for one row, and what it was drawn from. The virtual
/// list reuses row widgets for other rows as it scrolls, so the cache is
/// keyed by the row's content, not by the widget.
#[derive(Default)]
struct RowCache {
    cache: canvas::Cache,
    drawn: RefCell<Option<Key>>,
}

/// Everything the drawing depends on besides the size (the cache itself
/// redraws when the size changes).
type Key = (GraphRow, [Color; 6], Color);

impl<M> canvas::Program<M> for RowGraph<'_> {
    type State = RowCache;

    fn draw(
        &self,
        state: &RowCache,
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let same = state
            .drawn
            .borrow()
            .as_ref()
            .is_some_and(|(row, colors, bg)| {
                row == self.row && *colors == self.colors && *bg == self.background
            });
        if !same {
            state.cache.clear();
            *state.drawn.borrow_mut() = Some((self.row.clone(), self.colors, self.background));
        }
        vec![state.cache.draw(renderer, bounds.size(), |frame| {
            self.draw_row(frame, bounds.height)
        })]
    }
}

impl RowGraph<'_> {
    fn draw_row(&self, frame: &mut Frame, h: f32) {
        let y = |a: Anchor| match a {
            Anchor::Top => 0.0,
            Anchor::Center => h / 2.0,
            Anchor::Bottom => h,
        };
        let visible = |lane: usize| lane < MAX_LANES;

        for e in &self.row.edges {
            if !visible(e.from_lane) && !visible(e.to_lane) {
                continue;
            }
            let (x0, y0) = (x(e.from_lane), y(e.from));
            let (x1, y1) = (x(e.to_lane), y(e.to));
            let path = if e.from_lane == e.to_lane {
                Path::line(Point::new(x0, y0), Point::new(x1, y1))
            } else {
                // Vertical tangents at both ends make lane changes read as
                // smooth branches instead of zig-zags.
                let mid = (y0 + y1) / 2.0;
                Path::new(|b| {
                    b.move_to(Point::new(x0, y0));
                    b.bezier_curve_to(Point::new(x0, mid), Point::new(x1, mid), Point::new(x1, y1));
                })
            };
            frame.stroke(
                &path,
                Stroke::default()
                    .with_color(self.color(e.color))
                    .with_width(1.6),
            );
        }

        for a in &self.row.arrows {
            if !visible(a.from_lane) && !visible(a.to_lane) {
                continue;
            }
            let start = Point::new(x(a.from_lane), y(a.from));
            let full = Point::new(x(a.to_lane), y(a.to));
            // Stop short of the row edge so the head reads as an end, not
            // as a line running into the next row.
            let (dx, dy) = (full.x - start.x, full.y - start.y);
            let end = Point::new(start.x + dx * ARROW_REACH, start.y + dy * ARROW_REACH);
            let color = self.color(a.color);
            frame.stroke(
                &Path::line(start, end),
                Stroke::default().with_color(color).with_width(1.6),
            );
            let len = dx.hypot(dy).max(f32::EPSILON);
            let (ux, uy) = (dx / len, dy / len);
            let base = Point::new(end.x - ux * ARROW_HEAD, end.y - uy * ARROW_HEAD);
            let half = ARROW_HEAD * 0.6;
            let head = Path::new(|b| {
                b.move_to(end);
                b.line_to(Point::new(base.x - uy * half, base.y + ux * half));
                b.line_to(Point::new(base.x + uy * half, base.y - ux * half));
                b.close();
            });
            frame.fill(&head, color);
        }

        if visible(self.row.lane) {
            let center = Point::new(x(self.row.lane), h / 2.0);
            let color = self.color(self.row.color);
            if self.row.is_merge {
                frame.fill(&Path::circle(center, 4.0), color);
                frame.fill(&Path::circle(center, 2.0), self.background);
            } else {
                frame.fill(&Path::circle(center, 3.6), color);
            }
        }
    }
}

/// A canvas drawing one row of the graph, `width` px wide and as tall as
/// its parent.
pub fn row_graph<'a, M: 'a>(
    row: &'a GraphRow,
    colors: [Color; 6],
    background: Color,
    width: f32,
) -> Element<'a, M> {
    canvas::Canvas::new(RowGraph {
        row,
        colors,
        background,
    })
    .width(Length::Fixed(width))
    .height(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_grows_with_lanes_and_is_capped() {
        assert!(width_for(1) < width_for(3));
        assert_eq!(width_for(MAX_LANES), width_for(MAX_LANES + 20));
        // Far narrower than a two-characters-per-lane text graph at 13pt.
        assert!(width_for(8) < 8.0 * 2.0 * 13.0 * 0.6);
    }
}
