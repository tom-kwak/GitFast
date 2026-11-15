//! Remembers where the mouse was last pressed (in window coordinates) and
//! which text field has keyboard focus.
//!
//! Wrapped around the whole app, it sees every press before any widget
//! (and before scrollables translate the cursor), so a context menu can
//! open exactly where the user clicked without a message per mouse move.
//! iced has no focus events, so the focused field is looked up on every
//! key press (before the key is handled); the app reads it to decide
//! whether arrow keys go to a text field or move the selection of the
//! active panel.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use iced::advanced::widget::operation::Focusable;
use iced::advanced::widget::{Id, Operation, Tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// Which text field has keyboard focus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Field {
    #[default]
    None,
    /// A focused field without an id.
    Unnamed,
    Named(Id),
}

impl Field {
    pub fn is(&self, id: &'static str) -> bool {
        *self == Self::Named(Id::new(id))
    }
}

/// Shared last-press position (two `f32`s packed in one atomic) and focus.
#[derive(Debug, Clone, Default)]
pub struct Pointer {
    press: Arc<AtomicU64>,
    /// Focused field when the last key was pressed.
    focus: Arc<Mutex<Field>>,
}

impl Pointer {
    /// The field that had focus when the last key was pressed (so a key a
    /// field handled by giving up focus, like Esc, still counts as the
    /// field's).
    pub fn field_at_key(&self) -> Field {
        self.focus
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_field(&self, field: Field) {
        *self.focus.lock().unwrap_or_else(PoisonError::into_inner) = field;
    }

    pub fn last_press(&self) -> Point {
        let bits = self.press.load(Ordering::Relaxed);
        Point::new(
            f32::from_bits((bits >> 32) as u32),
            f32::from_bits(bits as u32),
        )
    }

    fn set(&self, p: Point) {
        let bits = (u64::from(p.x.to_bits()) << 32) | u64::from(p.y.to_bits());
        self.press.store(bits, Ordering::Relaxed);
    }

    /// Wraps `content` so presses inside it are recorded.
    pub fn track<'a, M: 'a>(&self, content: impl Into<Element<'a, M>>) -> Element<'a, M> {
        Element::new(Tracker {
            content: content.into(),
            pointer: self.clone(),
        })
    }
}

/// Finds the focused text field.
struct FindField(Field);

impl Operation for FindField {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        if state.is_focused() {
            self.0 = id.map_or(Field::Unnamed, |id| Field::Named(id.clone()));
        }
    }
}

fn find_field<M>(
    content: &mut Element<'_, M>,
    tree: &mut Tree,
    layout: Layout<'_>,
    renderer: &iced::Renderer,
) -> Field {
    let mut op = FindField(Field::None);
    content
        .as_widget_mut()
        .operate(&mut tree.children[0], layout, renderer, &mut op);
    op.0
}

struct Tracker<'a, M> {
    content: Element<'a, M>,
    pointer: Pointer,
}

impl<M> Widget<M, iced::Theme, iced::Renderer> for Tracker<'_, M> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
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
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(_)) => {
                if let Some(p) = cursor.position() {
                    self.pointer.set(p);
                }
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed { .. }) => {
                let field = find_field(&mut self.content, tree, layout, renderer);
                self.pointer.set_field(field);
            }
            _ => {}
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_and_unpacks_positions() {
        let p = Pointer::default();
        p.set(Point::new(123.5, -4.25));
        assert_eq!(p.last_press(), Point::new(123.5, -4.25));
    }

    #[test]
    fn names_the_focused_field() {
        let p = Pointer::default();
        assert_eq!(p.field_at_key(), Field::None);
        p.set_field(Field::Named(Id::new("prompt")));
        assert!(p.field_at_key().is("prompt"));
        assert!(!Field::Unnamed.is("prompt"));
    }
}
