//! Monospace artwork based on the logo export, with an animated crab.

use std::cell::Cell;
use std::f32::consts::TAU;

use iced::time::{Duration, Instant};
use iced::widget::canvas::{self, Cache, Frame, Geometry, Text};
use iced::{Color, Element, Font, Point, Rectangle, Renderer, Theme, alignment, mouse, window};

use crate::theme::Tokens;

const DESIGN_SIZE: f32 = 512.0;
const SIZE: f32 = 128.0;
const REDRAW_INTERVAL: Duration = Duration::from_millis(40);
const CYCLE: Duration = Duration::from_millis(2400);
const TRAVEL_STEPS: usize = 5;

// The source art uses two dot grids. The 'o' cells become asterisks, and
// the '.' cells become middle dots for the dim background grid.
const LOGO: [&str; 20] = [
    "....................",
    "....................",
    "....................",
    "....................",
    ".............ooooo..",
    ".............oo.....",
    ".....ooooo..oooooo..",
    "....oo...oo..oo.....",
    "...oo...oo..oo......",
    "...oo...oo..oo......",
    "...oo...oo..oo......",
    "....oooooo..oo......",
    "........oo..........",
    ".......oo...........",
    "..oo...oo...........",
    "...ooooo............",
    "....................",
    "..............",
    "..............",
    "..............",
];

const CRAB: [&str; 6] = [
    "oo   o o   oo",
    "o o  o o  o o",
    " oo ooooo oo",
    "  ooooooooo",
    "   ooooooo",
    " o o o o o o",
];

pub(super) fn view<Message: 'static>(tokens: &Tokens) -> Element<'static, Message> {
    canvas::Canvas::new(Logo {
        grid: tokens.border,
        letters: tokens.accent(),
        crab: tokens.warning(),
    })
    .width(SIZE)
    .height(SIZE)
    .into()
}

#[derive(Clone, Copy, PartialEq)]
struct Logo {
    grid: Color,
    letters: Color,
    crab: Color,
}

#[derive(Default)]
struct State {
    start: Option<Instant>,
    elapsed: Duration,
    colors: Cell<Option<Logo>>,
    lettering: Cache,
    crab: [Cache; 2 * TRAVEL_STEPS + 1],
}

impl<Message> canvas::Program<Message> for Logo {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &canvas::Event,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let canvas::Event::Window(window::Event::RedrawRequested(now)) = event else {
            return None;
        };
        let start = *state.start.get_or_insert(*now);
        state.elapsed = now.saturating_duration_since(start);
        // Redraw the widget without rebuilding the welcome view or sending an App message.
        Some(canvas::Action::request_redraw_at(*now + REDRAW_INTERVAL))
    }

    fn draw(
        &self,
        state: &State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        // Geometry includes glyph colors, so update both layers on a live theme change.
        if state.colors.replace(Some(*self)) != Some(*self) {
            state.lettering.clear();
            for frame in &state.crab {
                frame.clear();
            }
        }

        let size = bounds.size();
        let lettering = state.lettering.draw(renderer, size, |frame| {
            frame.scale(size.width / DESIGN_SIZE);
            for (row, dots) in LOGO.iter().enumerate() {
                let origin = Point::new(66.0, 66.0 + row as f32 * 20.0);
                draw_text_row(frame, dots, '.', origin, 20.0, self.grid);
                draw_text_row(frame, dots, 'o', origin, 20.0, self.letters);
            }
        });
        let (index, offset) = crab_position(state.elapsed);
        let crab = state.crab[index].draw(renderer, size, |frame| {
            frame.scale(size.width / DESIGN_SIZE);
            for (row, dots) in CRAB.iter().enumerate() {
                let origin = Point::new(338.0 + offset, 395.0 + row as f32 * 9.0);
                draw_text_row(frame, dots, 'o', origin, 9.0, self.crab);
            }
        });
        vec![lettering, crab]
    }
}

fn draw_text_row(
    frame: &mut Frame,
    dots: &str,
    mark: char,
    origin: Point,
    spacing: f32,
    color: Color,
) {
    if !dots.contains(mark) {
        return;
    }
    let (glyph, baseline_offset) = if mark == '.' {
        ('·', 0.0)
    } else {
        ('*', 0.12)
    };
    frame.fill_text(Text {
        content: dots
            .chars()
            .map(|dot| if dot == mark { glyph } else { ' ' })
            .collect(),
        position: Point::new(
            origin.x - spacing / 2.0,
            origin.y + spacing * baseline_offset,
        ),
        size: (spacing / 0.61).into(),
        align_y: alignment::Vertical::Center,
        font: Font::MONOSPACE,
        color,
        ..Text::default()
    });
}

fn crab_position(elapsed: Duration) -> (usize, f32) {
    let phase = (elapsed.as_secs_f32() % CYCLE.as_secs_f32()) / CYCLE.as_secs_f32();
    let step = ((phase * TAU).sin() * TRAVEL_STEPS as f32).round();
    ((step + TRAVEL_STEPS as f32) as usize, step * 4.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crab_moves_right_then_left_and_returns_to_its_original_position() {
        assert_eq!(crab_position(Duration::ZERO), (5, 0.0));
        assert_eq!(crab_position(CYCLE / 4), (10, 20.0));
        assert_eq!(crab_position(CYCLE / 2), (5, 0.0));
        assert_eq!(crab_position(CYCLE * 3 / 4), (0, -20.0));
        assert_eq!(crab_position(CYCLE), (5, 0.0));
    }
}
