//! A row whose items can be dragged into another order, like browser tabs.
//!
//! A press that moves a few pixels picks the item up: it follows the
//! cursor and the others slide aside to show where it will land. Letting
//! go reports `on_drop(from, to)`; the app reorders its own list. A press
//! that does not move stays a click for the item itself (its buttons get
//! it as usual). Esc or the window losing focus puts the item back.
//! While an item is held its children see no mouse events, so nothing
//! under the cursor lights up, opens a tooltip or takes the release as a
//! click.

use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{
    Clipboard, Layout, Renderer as _, Shell, Widget, layout, mouse, overlay, renderer,
};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, keyboard, window};

/// How far (pixels) a press moves before it becomes a drag.
const THRESHOLD: f32 = 4.0;

pub struct Reorder<'a, M> {
    items: Vec<Element<'a, M>>,
    spacing: f32,
    on_pick: Option<Box<dyn Fn(usize) -> M + 'a>>,
    on_drop: Option<Box<dyn Fn(usize, usize) -> M + 'a>>,
}

impl<M> std::fmt::Debug for Reorder<'_, M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reorder").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum State {
    #[default]
    Idle,
    /// Pressed on item `index` at `x`, not moved far enough yet.
    Pressed { index: usize, x: f32 },
    /// Item `index` follows the cursor, `dx` from where it was.
    Dragging { index: usize, x: f32, dx: f32 },
}

impl<'a, M> Reorder<'a, M> {
    pub fn new(items: Vec<Element<'a, M>>) -> Self {
        Self {
            items,
            spacing: 0.0,
            on_pick: None,
            on_drop: None,
        }
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// An item was picked up (browser tabs are shown at that moment).
    pub fn on_pick(mut self, f: impl Fn(usize) -> M + 'a) -> Self {
        self.on_pick = Some(Box::new(f));
        self
    }

    /// An item was let go at another place: `(from, to)`.
    pub fn on_drop(mut self, f: impl Fn(usize, usize) -> M + 'a) -> Self {
        self.on_drop = Some(Box::new(f));
        self
    }
}

/// Where the item `from` lands when its left edge is at `left`. Like
/// browser tabs, it passes an item on its right once its right edge is
/// past that item's middle, and one on its left once its left edge is, so
/// it reaches either end even when it is wider than the item there.
/// `spans` are each item's `(x, width)`.
fn landing(spans: &[(f32, f32)], from: usize, left: f32) -> usize {
    let right = left + spans[from].1;
    let middle = |&(x, w): &(f32, f32)| x + w / 2.0;
    let passed_right = spans[from + 1..]
        .iter()
        .filter(|s| right > middle(s))
        .count();
    let passed_left = spans[..from].iter().filter(|s| left < middle(s)).count();
    from + passed_right - passed_left
}

/// How far item `j` slides aside while `from` is held over slot `to`.
fn slide(j: usize, from: usize, to: usize, by: f32) -> f32 {
    if from < j && j <= to {
        -by
    } else if to <= j && j < from {
        by
    } else {
        0.0
    }
}

/// `dx` kept so the held item stays within the row.
fn clamp_dx(spans: &[(f32, f32)], from: usize, dx: f32) -> f32 {
    let (Some(first), Some(last)) = (spans.first(), spans.last()) else {
        return 0.0;
    };
    let (x, w) = spans[from];
    let min = first.0 - x;
    let max = (last.0 + last.1) - (x + w);
    dx.clamp(min, max.max(min))
}

fn spans(layout: Layout<'_>) -> Vec<(f32, f32)> {
    layout
        .children()
        .map(|c| (c.bounds().x, c.bounds().width))
        .collect()
}

impl<M> Widget<M, iced::Theme, iced::Renderer> for Reorder<'_, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        self.items.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.items);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let child_limits =
            layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, limits.max().height));
        let mut nodes: Vec<layout::Node> = self
            .items
            .iter_mut()
            .zip(&mut tree.children)
            .map(|(item, tree)| item.as_widget_mut().layout(tree, renderer, &child_limits))
            .collect();
        let height = nodes.iter().map(|n| n.size().height).fold(0.0, f32::max);
        let mut x = 0.0;
        for node in &mut nodes {
            let size = node.size();
            node.move_to_mut(Point::new(x, (height - size.height) / 2.0));
            x += size.width + self.spacing;
        }
        let width = (x - self.spacing).max(0.0);
        layout::Node::with_children(
            limits.resolve(Length::Shrink, Length::Shrink, Size::new(width, height)),
            nodes,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        for ((item, tree), layout) in self
            .items
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            item.as_widget_mut()
                .operate(tree, layout, renderer, operation);
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let state = *tree.state.downcast_ref::<State>();
        let mut next = state;
        // Children see the event as usual unless an item is held.
        let mut forward = Some(cursor);
        match (state, event) {
            (State::Idle, Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))) => {
                if let Some(p) = cursor.position()
                    && let Some(index) = layout.children().position(|c| c.bounds().contains(p))
                {
                    next = State::Pressed { index, x: p.x };
                }
            }
            (State::Pressed { index, x }, Event::Mouse(mouse::Event::CursorMoved { position }))
                if (position.x - x).abs() >= THRESHOLD =>
            {
                next = State::Dragging { index, x, dx: 0.0 };
                if let Some(f) = &self.on_pick {
                    shell.publish(f(index));
                }
                // Let go of whatever the press started inside the item.
                forward = Some(mouse::Cursor::Unavailable);
            }
            (State::Pressed { .. }, Event::Mouse(mouse::Event::ButtonReleased(_))) => {
                next = State::Idle;
            }
            (
                State::Dragging { index, x, .. },
                Event::Mouse(mouse::Event::CursorMoved { position }),
            ) => {
                next = State::Dragging {
                    index,
                    x,
                    dx: clamp_dx(&spans(layout), index, position.x - x),
                };
                shell.request_redraw();
                forward = None;
            }
            (State::Dragging { index, dx, .. }, Event::Mouse(mouse::Event::ButtonReleased(_))) => {
                next = State::Idle;
                let spans = spans(layout);
                let to = landing(&spans, index, spans[index].0 + dx);
                if to != index
                    && let Some(f) = &self.on_drop
                {
                    shell.publish(f(index, to));
                }
                shell.capture_event();
                shell.request_redraw();
                // Buttons pressed when the drag began reset without a click.
                forward = Some(mouse::Cursor::Unavailable);
            }
            (
                State::Dragging { .. },
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                })
                | Event::Window(window::Event::Unfocused),
            ) => {
                next = State::Idle;
                shell.capture_event();
                shell.request_redraw();
                forward = None;
            }
            (State::Dragging { .. }, Event::Mouse(_)) => forward = None,
            _ => {}
        }
        *tree.state.downcast_mut::<State>() = next;
        let Some(cursor) = forward else {
            return;
        };
        for ((item, tree), layout) in self
            .items
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            item.as_widget_mut().update(
                tree, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if let State::Dragging { .. } = tree.state.downcast_ref::<State>() {
            return mouse::Interaction::Grabbing;
        }
        self.items
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((item, tree), layout)| {
                item.as_widget()
                    .mouse_interaction(tree, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let held = match *tree.state.downcast_ref::<State>() {
            State::Dragging { index, dx, .. } => Some((index, dx)),
            _ => None,
        };
        let Some((from, dx)) = held else {
            for ((item, tree), layout) in
                self.items.iter().zip(&tree.children).zip(layout.children())
            {
                item.as_widget()
                    .draw(tree, renderer, theme, style, layout, cursor, viewport);
            }
            return;
        };
        let spans = spans(layout);
        let to = landing(&spans, from, spans[from].0 + dx);
        let by = spans[from].1 + self.spacing;
        // No hover look while an item is held.
        let cursor = mouse::Cursor::Unavailable;
        for (j, ((item, tree), layout)) in self
            .items
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .enumerate()
        {
            if j == from {
                continue;
            }
            let shift = Vector::new(slide(j, from, to, by), 0.0);
            renderer.with_translation(shift, |r| {
                item.as_widget()
                    .draw(tree, r, theme, style, layout, cursor, viewport);
            });
        }
        // The held item on top, under the cursor.
        if let (Some(item), Some(tree), Some(layout)) = (
            self.items.get(from),
            tree.children.get(from),
            layout.children().nth(from),
        ) {
            renderer.with_layer(*viewport, |r| {
                r.with_translation(Vector::new(dx, 0.0), |r| {
                    item.as_widget()
                        .draw(tree, r, theme, style, layout, cursor, viewport);
                });
            });
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, iced::Theme, iced::Renderer>> {
        // No tooltips while an item is held.
        if let State::Dragging { .. } = tree.state.downcast_ref::<State>() {
            return None;
        }
        overlay::from_children(
            &mut self.items,
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, M: 'a> From<Reorder<'a, M>> for Element<'a, M> {
    fn from(reorder: Reorder<'a, M>) -> Self {
        Element::new(reorder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Three items 100px wide, 1px apart.
    const SPANS: [(f32, f32); 3] = [(0.0, 100.0), (101.0, 100.0), (202.0, 100.0)];

    #[test]
    fn a_held_item_lands_past_the_middles_it_has_crossed() {
        assert_eq!(landing(&SPANS, 0, 0.0), 0, "not moved");
        assert_eq!(landing(&SPANS, 0, 40.0), 0, "not past the next middle");
        assert_eq!(landing(&SPANS, 0, 60.0), 1);
        assert_eq!(landing(&SPANS, 0, 202.0), 2);
        assert_eq!(landing(&SPANS, 2, 160.0), 2, "not past the previous middle");
        assert_eq!(landing(&SPANS, 2, 90.0), 1);
        assert_eq!(landing(&SPANS, 2, 0.0), 0);
    }

    #[test]
    fn a_wide_item_reaches_either_end_past_a_narrow_one() {
        // 100px item next to a 40px one: pushed to the end of the row it
        // takes the last place.
        let spans = [(0.0, 100.0), (101.0, 40.0)];
        let dx = clamp_dx(&spans, 0, 500.0);
        assert_eq!(landing(&spans, 0, dx), 1);
        // And back to the front from the other side.
        let spans = [(0.0, 40.0), (41.0, 100.0)];
        let dx = clamp_dx(&spans, 1, -500.0);
        assert_eq!(landing(&spans, 1, 41.0 + dx), 0);
    }

    #[test]
    fn the_others_slide_aside_toward_where_the_item_came_from() {
        // 0 held over slot 2: 1 and 2 slide left.
        assert_eq!(slide(1, 0, 2, 101.0), -101.0);
        assert_eq!(slide(2, 0, 2, 101.0), -101.0);
        // 2 held over slot 0: 0 and 1 slide right.
        assert_eq!(slide(0, 2, 0, 101.0), 101.0);
        assert_eq!(slide(1, 2, 0, 101.0), 101.0);
        // Not between: stays.
        assert_eq!(slide(2, 0, 1, 101.0), 0.0);
    }

    #[test]
    fn a_held_item_stays_within_the_row() {
        assert_eq!(clamp_dx(&SPANS, 0, -50.0), 0.0);
        assert_eq!(clamp_dx(&SPANS, 0, 500.0), 202.0);
        assert_eq!(clamp_dx(&SPANS, 1, 30.0), 30.0);
        assert_eq!(clamp_dx(&SPANS[..1], 0, 30.0), 0.0);
    }
}
