//! Overview of changed rows alongside the code viewer's scrollbar.

use std::cell::RefCell;
use std::sync::Arc;

use iced::widget::canvas::{self, Geometry};
use iced::widget::{Container, container, space};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse};

use crate::theme::Tokens;

// Bound the number of marks even for a file with many alternating changes.
const SLOTS: usize = 512;
const TRACK_WIDTH: f32 = 14.0;
const MARK_WIDTH: f32 = 5.0;
const MARK_HEIGHT: f32 = 4.0;

#[derive(Debug, PartialEq)]
pub(super) struct Mark {
    position: f32,
    kinds: [bool; 2],
}

/// Summarize changes by their positions in the displayed rows.
pub(super) fn collect(rows: impl ExactSizeIterator<Item = [bool; 2]>) -> Arc<[Mark]> {
    let last_row = rows.len().saturating_sub(1).max(1);
    let mut slots = [[false; 2]; SLOTS];
    for (index, [added, removed]) in rows.enumerate() {
        if added || removed {
            let slot = index * (SLOTS - 1) / last_row;
            slots[slot][0] |= added;
            slots[slot][1] |= removed;
        }
    }
    slots
        .into_iter()
        .enumerate()
        .filter_map(|(slot, kinds)| {
            #[allow(clippy::cast_precision_loss)] // Both values are at most SLOTS.
            let position = slot as f32 / (SLOTS - 1) as f32;
            (kinds[0] || kinds[1]).then_some(Mark { position, kinds })
        })
        .collect()
}

struct Track<'a> {
    marks: &'a Arc<[Mark]>,
    colors: [Color; 2],
}

type DrawingKey = (Arc<[Mark]>, [Color; 2]);

#[derive(Default)]
struct DrawCache {
    geometry: canvas::Cache,
    key: RefCell<Option<DrawingKey>>,
}

impl<M> canvas::Program<M> for Track<'_> {
    type State = DrawCache;

    fn draw(
        &self,
        state: &DrawCache,
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let same = state.key.borrow().as_ref().is_some_and(|(marks, colors)| {
            Arc::ptr_eq(marks, self.marks) && *colors == self.colors
        });
        if !same {
            state.geometry.clear();
            *state.key.borrow_mut() = Some((self.marks.clone(), self.colors));
        }
        vec![state.geometry.draw(renderer, bounds.size(), |frame| {
            let height = bounds.height.min(MARK_HEIGHT);
            for mark in self.marks.iter() {
                let y = mark.position * (bounds.height - height);
                for (lane, present) in mark.kinds.into_iter().enumerate() {
                    if present {
                        let x = if lane == 0 {
                            1.0
                        } else {
                            TRACK_WIDTH - MARK_WIDTH - 1.0
                        };
                        frame.fill_rectangle(
                            Point::new(x, y),
                            Size::new(MARK_WIDTH, height),
                            self.colors[lane],
                        );
                    }
                }
            }
        })]
    }
}

pub(super) fn view<'a, M: 'a>(t: &'a Tokens, marks: &'a Arc<[Mark]>) -> Element<'a, M> {
    track(
        t,
        canvas::Canvas::new(Track {
            marks,
            colors: [t.added(), t.removed()],
        })
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .into()
}

/// Continues the track through the caption without painting marks there.
pub(super) fn cap<'a, M: 'a>(t: &'a Tokens, height: f32) -> Element<'a, M> {
    track(t, space().height(height)).into()
}

fn track<'a, M: 'a>(t: &'a Tokens, content: impl Into<Element<'a, M>>) -> Container<'a, M> {
    container(content)
        .width(Length::Fixed(TRACK_WIDTH))
        .style(move |_| container::Style::default().background(t.surface))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearby_additions_and_deletions_share_a_position_without_hiding_each_other() {
        let mut rows = vec![[false; 2]; 2048];
        rows[0] = [true, false];
        rows[1] = [false, true];
        rows[2047] = [false, true];
        assert_eq!(
            &*collect(rows.into_iter()),
            &[
                Mark {
                    position: 0.0,
                    kinds: [true, true]
                },
                Mark {
                    position: 1.0,
                    kinds: [false, true]
                },
            ],
        );
        assert!(collect([[false; 2]].into_iter()).is_empty());
    }
}
