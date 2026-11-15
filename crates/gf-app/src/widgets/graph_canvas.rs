//! Commit graph row drawn as vector lines on a canvas.
//!
//! Each row draws its [`Edge`]s from the very top to the very bottom of the
//! row, so lines join seamlessly with the rows above and below. Lane
//! changes are smooth curves; lanes are only [`LANE_SPACING`] px apart, far
//! narrower than the two characters per lane of a text graph.

use gf_git::graph::{Anchor, GraphRow};
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme, mouse};

/// Horizontal distance between lanes.
pub const LANE_SPACING: f32 = 11.0;
/// Space left of the first lane.
const PAD: f32 = 7.0;
/// Lanes drawn at most; wider graphs are clipped with a fade marker.
pub const MAX_LANES: usize = 14;

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

impl<M> canvas::Program<M> for RowGraph<'_> {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let h = bounds.height;
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
        vec![frame.into_geometry()]
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
