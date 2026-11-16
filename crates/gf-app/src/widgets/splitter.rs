//! A vertical divider that resizes the panes on either side by dragging.
//!
//! It only reports movement — `on_drag(dx)` with the horizontal distance in
//! pixels from where the drag started, `on_release` when it ends and
//! `on_reset` on double-click — so the app keeps one width and every view
//! that shows the split uses it. The app applies `dx` to the width it had
//! when the drag started, so the divider stays under the cursor even after
//! hitting a limit. Mouse moves are followed outside the bar while the
//! button is held.

use std::time::{Duration, Instant};

use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, renderer};
use iced::{Background, Border, Color, Element, Event, Length, Rectangle, Size};

/// Width of the bar (and of the gap it takes between the panes).
pub const WIDTH: f32 = 6.0;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

pub struct Splitter<M> {
    on_drag: Box<dyn Fn(f32) -> M>,
    on_press: Option<M>,
    on_release: Option<M>,
    on_reset: Option<M>,
    line: Color,
    active: Color,
}

impl<M> std::fmt::Debug for Splitter<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Splitter").finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct State {
    /// Cursor x where the drag started.
    dragging: Option<f32>,
    /// Last distance reported, to skip repeats.
    reported: f32,
    last_press: Option<Instant>,
}

impl<M> Splitter<M> {
    pub fn new(on_drag: impl Fn(f32) -> M + 'static, line: Color, active: Color) -> Self {
        Self {
            on_drag: Box::new(on_drag),
            on_press: None,
            on_release: None,
            on_reset: None,
            line,
            active,
        }
    }

    /// A drag starts: the app notes the width `on_drag` distances apply to.
    pub fn on_press(mut self, message: M) -> Self {
        self.on_press = Some(message);
        self
    }

    pub fn on_release(mut self, message: M) -> Self {
        self.on_release = Some(message);
        self
    }

    pub fn on_reset(mut self, message: M) -> Self {
        self.on_reset = Some(message);
        self
    }
}

impl<M: Clone> Widget<M, iced::Theme, iced::Renderer> for Splitter<M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(WIDTH), Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, WIDTH, Length::Fill)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(p) = cursor.position_over(layout.bounds()) else {
                    return;
                };
                let now = Instant::now();
                let double = state
                    .last_press
                    .is_some_and(|t| now.duration_since(t) < DOUBLE_CLICK);
                state.last_press = Some(now);
                if double {
                    state.dragging = None;
                    if let Some(m) = &self.on_reset {
                        shell.publish(m.clone());
                    }
                } else {
                    state.dragging = Some(p.x);
                    state.reported = 0.0;
                    if let Some(m) = &self.on_press {
                        shell.publish(m.clone());
                    }
                }
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(start) = state.dragging {
                    let dx = (position.x - start).round();
                    if (dx - state.reported).abs() >= 1.0 {
                        state.reported = dx;
                        shell.publish((self.on_drag)(dx));
                    }
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.dragging.is_some() =>
            {
                state.dragging = None;
                if let Some(m) = &self.on_release {
                    shell.publish(m.clone());
                }
                shell.capture_event();
                shell.request_redraw();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let dragging = tree.state.downcast_ref::<State>().dragging.is_some();
        if dragging || cursor.is_over(layout.bounds()) {
            mouse::Interaction::ResizingHorizontally
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let dragging = tree.state.downcast_ref::<State>().dragging.is_some();
        let hot = dragging || cursor.is_over(bounds);
        let (color, width) = if hot {
            (self.active, 2.0)
        } else {
            (self.line, 1.0)
        };
        let line = Rectangle {
            x: bounds.center_x() - width / 2.0,
            width,
            ..bounds
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds: line,
                border: Border::default(),
                ..renderer::Quad::default()
            },
            Background::Color(color),
        );
    }
}

impl<'a, M: Clone + 'a> From<Splitter<M>> for Element<'a, M> {
    fn from(s: Splitter<M>) -> Self {
        Element::new(s)
    }
}
