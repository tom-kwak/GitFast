//! A control with a list that drops down over what is below it (a
//! dropdown), e.g. the branch suggestions of a dialog field.
//!
//! [`dropdown`] lays out only `base`; the open `list` is drawn as an
//! overlay right under it, as wide as it, on top of everything (the rows
//! below do not move). Built like iced's tooltip: the list lives in the
//! base's widget tree, so its buttons keep working as usual.

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, mouse};

/// `base` with `list` (when `Some`) dropped down under it.
pub fn dropdown<'a, M: 'a>(
    base: impl Into<Element<'a, M>>,
    list: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let open = list.is_some();
    Element::new(Dropdown {
        base: base.into(),
        list: list.unwrap_or_else(|| iced::widget::space().into()),
        open,
    })
}

struct Dropdown<'a, M> {
    base: Element<'a, M>,
    list: Element<'a, M>,
    open: bool,
}

impl<M> Widget<M, iced::Theme, iced::Renderer> for Dropdown<'_, M> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.base), Tree::new(&self.list)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[self.base.as_widget(), self.list.as_widget()]);
    }

    fn size(&self) -> Size<Length> {
        self.base.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.base.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.base
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
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
        self.base.as_widget_mut().update(
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
        self.base.as_widget().mouse_interaction(
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
        self.base.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        // Focus and cursor operations reach the base's text field by id.
        self.base
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, iced::Theme, iced::Renderer>> {
        let mut children = tree.children.iter_mut();
        let base_tree = children.next()?;
        let list_tree = children.next()?;
        let base =
            self.base
                .as_widget_mut()
                .overlay(base_tree, layout, renderer, viewport, translation);
        let bounds = layout.bounds();
        let list = self.open.then(|| {
            overlay::Element::new(Box::new(List {
                list: &mut self.list,
                tree: list_tree,
                position: Point::new(bounds.x, bounds.y + bounds.height) + translation,
                width: bounds.width,
            }))
        });
        let all: Vec<_> = base.into_iter().chain(list).collect();
        (!all.is_empty()).then(|| overlay::Group::with_children(all).overlay())
    }
}

/// The dropped-down list, drawn over the rest.
struct List<'a, 'b, M> {
    list: &'b mut Element<'a, M>,
    tree: &'b mut Tree,
    position: Point,
    width: f32,
}

impl<M> overlay::Overlay<M, iced::Theme, iced::Renderer> for List<'_, '_, M> {
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        let room = (bounds.height - self.position.y).max(0.0);
        let limits = layout::Limits::new(Size::new(self.width, 0.0), Size::new(self.width, room));
        self.list
            .as_widget_mut()
            .layout(self.tree, renderer, &limits)
            .translate(Vector::new(self.position.x, self.position.y))
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.list.as_widget().draw(
            self.tree,
            renderer,
            theme,
            style,
            layout,
            cursor,
            &layout.bounds(),
        );
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
    ) {
        self.list.as_widget_mut().update(
            self.tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &layout.bounds(),
        );
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.list.as_widget().mouse_interaction(
            self.tree,
            layout,
            cursor,
            &layout.bounds(),
            renderer,
        )
    }
}
