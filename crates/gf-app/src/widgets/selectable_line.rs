//! A line of code whose characters can be selected with the mouse, as in a
//! text viewer: it finds the character under the mouse and draws the
//! selection behind the text from the same text layout that draws the
//! text, so the selection lines up with every glyph — wide (Hangul) and
//! fallback-font ones included — however long the line.
//!
//! The line is laid out as a fixed-width gutter (line numbers), the marker
//! (`+` / `-` / ` `) and then the code. Positions count the selectable text
//! (see `features::diff_view::select`): 0 before the marker, 1 at the start
//! of the code, `1 + n` after `n` characters of the raw code (a tab is one
//! character of code, drawn as four spaces).
//!
//! Only a press, and — while the owner says a drag is on — reaching another
//! position, send a message: hovering never rebuilds the view.

use iced::advanced::text::{self, Paragraph as _};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Color, Element, Event, Length, Pixels, Point, Rectangle, Size, Vector};
use unicode_segmentation::UnicodeSegmentation;

use super::format;

type Paragraph = <iced::Renderer as text::Renderer>::Paragraph;
type OnPointer<'a, M> = Box<dyn Fn(TextPointer) -> M + 'a>;

/// A caret boundary and the character under the pointer (one past the
/// text in trailing space). They differ on the right half of a glyph.
#[derive(Debug, Clone, Copy)]
pub struct TextPointer {
    pub caret: usize,
    pub hit: usize,
    pub point: Point,
    pub button: mouse::Button,
}

/// Where things are on the line, in pixels from its left edge.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    /// Left edge of the marker.
    pub marker_x: f32,
    /// Left edge of the code.
    pub code_x: f32,
    /// Width of one monospace cell (the marker's, a selected line break's).
    pub cell: f32,
    /// Font size of the code.
    pub size: f32,
}

pub struct SelectableLine<'a, M> {
    content: Element<'a, M>,
    raw: &'a str,
    geometry: Geometry,
    background: Option<Color>,
    selection: (Color, Color),
    marker: char,
    /// Selected `(start, end, line break too)` in positions.
    spans: Vec<(usize, usize, bool)>,
    on_press: OnPointer<'a, M>,
    on_drag: Option<OnPointer<'a, M>>,
}

impl<M> std::fmt::Debug for SelectableLine<'_, M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectableLine")
            .field("raw", &self.raw)
            .field("spans", &self.spans)
            .finish_non_exhaustive()
    }
}

/// `content` drawn over `background`, for the code `raw`; `on_press`
/// receives left and right presses on the row.
pub fn selectable_line<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    raw: &'a str,
    geometry: Geometry,
    on_press: impl Fn(TextPointer) -> M + 'a,
) -> SelectableLine<'a, M> {
    SelectableLine {
        content: content.into(),
        raw,
        geometry,
        background: None,
        selection: (Color::TRANSPARENT, Color::TRANSPARENT),
        marker: ' ',
        spans: Vec::new(),
        on_press: Box::new(on_press),
        on_drag: None,
    }
}

impl<'a, M: 'a> SelectableLine<'a, M> {
    pub fn background(mut self, color: Option<Color>) -> Self {
        self.background = color;
        self
    }

    /// The selected spans of this line, drawn in `color`.
    pub fn selected(
        mut self,
        spans: Vec<(usize, usize, bool)>,
        colors: (Color, Color),
        marker: char,
    ) -> Self {
        self.spans = spans;
        self.selection = colors;
        self.marker = marker;
        self
    }

    /// While `dragging`, also `on_drag(pos)` when the mouse reaches another
    /// position of this line.
    pub fn on_drag(mut self, dragging: bool, on_drag: impl Fn(TextPointer) -> M + 'a) -> Self {
        if dragging {
            self.on_drag = Some(Box::new(on_drag));
        }
        self
    }

    /// The position nearest to `x` (pixels from the line's left edge).
    fn pos_at(&self, shown: &str, paragraph: &Paragraph, x: f32) -> usize {
        let g = self.geometry;
        if x < g.marker_x + g.cell / 2.0 {
            return 0;
        }
        if x < g.code_x {
            return 1;
        }
        let at = Point::new(x - g.code_x, g.size * 0.6);
        let shown_chars = match paragraph.hit_test(at) {
            Some(hit) => {
                let byte = hit.cursor().min(shown.len());
                shown.char_indices().take_while(|(i, _)| *i < byte).count()
            }
            None => shown.chars().count(),
        };
        1 + raw_index(self.raw.strip_suffix('\r').unwrap_or(self.raw), shown_chars)
    }

    /// The x (from the line's left edge) of position `pos`.
    fn x_of(&self, shown: &str, paragraph: &Paragraph, pos: usize) -> f32 {
        let g = self.geometry;
        if pos == 0 {
            return g.marker_x;
        }
        let chars = shown_index(self.raw, pos - 1);
        let graphemes = shown
            .graphemes(true)
            .scan(0, |count, cluster| {
                *count += cluster.chars().count();
                Some(*count)
            })
            .take_while(|&end| end <= chars)
            .count();
        let dx = if graphemes == 0 {
            0.0
        } else {
            paragraph
                .grapheme_position(0, graphemes)
                .map_or_else(|| paragraph.min_width(), |p| p.x)
        };
        g.code_x + dx
    }

    fn pointer(
        &self,
        state: &mut State,
        p: Point,
        bounds: Rectangle,
        button: mouse::Button,
        renderer: &iced::Renderer,
    ) -> TextPointer {
        let (shown, paragraph) = state.lay_out(self.raw, self.geometry.size, renderer);
        let x = p.x - bounds.x;
        let caret = self.pos_at(shown, paragraph, x);
        let hit = if x < self.geometry.code_x {
            0
        } else if x < self.x_of(shown, paragraph, caret) {
            caret.saturating_sub(1).max(1)
        } else {
            caret
        };
        TextPointer {
            caret,
            hit,
            point: p,
            button,
        }
    }
}

/// Index in the drawn code (tabs as four spaces, a final `\r` hidden) of
/// raw character `n`.
fn shown_index(raw: &str, n: usize) -> usize {
    let count = raw.chars().count();
    raw.chars()
        .take(n)
        .enumerate()
        .map(|(i, c)| match c {
            '\t' => 4,
            '\r' if i + 1 == count => 0,
            _ => 1,
        })
        .sum()
}

/// The raw character at drawn index `shown` (the nearer end of a tab).
fn raw_index(raw: &str, shown: usize) -> usize {
    let mut at = 0;
    for (i, c) in raw.chars().enumerate() {
        let w = if c == '\t' { 4 } else { 1 };
        if shown < at + w {
            return if shown - at < w.div_ceil(2) { i } else { i + 1 };
        }
        at += w;
    }
    raw.chars().count()
}

/// The code laid out (the hit test and the selection need it; the text
/// widget keeps its own). Only made for a line that is pressed, dragged
/// over or selected, so scrolling past lines costs nothing extra.
#[derive(Default)]
struct State {
    laid_out: Option<(String, f32, String, Paragraph)>,
    /// The position last reported, so moving within it sends nothing.
    last: Option<(usize, usize)>,
}

impl State {
    fn paragraph(&self) -> Option<(&str, &Paragraph)> {
        self.laid_out
            .as_ref()
            .map(|(_, _, shown, p)| (shown.as_str(), p))
    }

    /// Lays out `raw` unless it already is, at this size.
    fn lay_out(&mut self, raw: &str, size: f32, renderer: &iced::Renderer) -> (&str, &Paragraph) {
        let stale = !matches!(&self.laid_out, Some((r, s, _, _)) if r == raw && *s == size);
        if stale {
            let shown = format::code_line(raw).into_owned();
            let paragraph = Paragraph::with_text(text::Text {
                content: shown.as_str(),
                bounds: Size::INFINITE,
                size: Pixels(size),
                line_height: text::LineHeight::default(),
                font: text::Renderer::default_font(renderer),
                align_x: text::Alignment::Default,
                align_y: iced::alignment::Vertical::Top,
                shaping: text::Shaping::default(),
                wrapping: text::Wrapping::None,
            });
            self.laid_out = Some((raw.to_owned(), size, shown, paragraph));
        }
        self.paragraph().expect("laid out above")
    }
}

impl<M> Widget<M, iced::Theme, iced::Renderer> for SelectableLine<'_, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

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
        let state = tree.state.downcast_mut::<State>();
        if self.spans.is_empty() {
            // Not drawn selected: a scrolled-in line may hold another
            // line's layout; drop it rather than keep a stale one.
            if !matches!(&state.laid_out, Some((r, _, _, _)) if r == self.raw) {
                state.laid_out = None;
            }
        } else {
            state.lay_out(self.raw, self.geometry.size, renderer);
        }
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
        let bounds = layout.bounds();
        let over = bounds
            .intersection(viewport)
            .and_then(|visible| cursor.position_over(visible));
        let state = tree.state.downcast_mut::<State>();
        match (event, over) {
            (
                Event::Mouse(mouse::Event::ButtonPressed(
                    button @ (mouse::Button::Left | mouse::Button::Right),
                )),
                Some(p),
            ) => {
                let pointer = self.pointer(state, p, bounds, *button, renderer);
                state.last = Some((pointer.caret, pointer.hit));
                shell.publish((self.on_press)(pointer));
                shell.capture_event();
                return;
            }
            (Event::Mouse(mouse::Event::CursorMoved { .. }), Some(p)) => {
                if let Some(on_drag) = &self.on_drag {
                    let pointer = self.pointer(state, p, bounds, mouse::Button::Left, renderer);
                    let pos = (pointer.caret, pointer.hit);
                    if state.last != Some(pos) {
                        state.last = Some(pos);
                        shell.publish(on_drag(pointer));
                    }
                }
            }
            // Left this line: coming back must report again.
            (Event::Mouse(mouse::Event::CursorMoved { .. }), None) => state.last = None,
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
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if cursor
            .position_over(layout.bounds())
            .is_some_and(|p| p.x >= layout.bounds().x + self.geometry.marker_x)
        {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::default()
        }
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
        use iced::advanced::Renderer as _;
        use iced::advanced::text::Renderer as _;
        let bounds = layout.bounds();
        if let Some(bg) = self.background {
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    ..renderer::Quad::default()
                },
                bg,
            );
        }
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
        if let Some((shown, paragraph)) = tree.state.downcast_ref::<State>().paragraph() {
            // Use the code and marker widgets' positions so a selected
            // glyph stays on the same baseline as its neighbors.
            let Some(row) = layout.children().next() else {
                return;
            };
            let mut cells = row.children().skip(2);
            let marker_y = cells
                .next()
                .and_then(|cell| cell.children().next())
                .map_or(bounds.y, |marker| marker.position().y);
            let Some(code) = cells.next() else {
                return;
            };
            for &(s, e, newline) in &self.spans {
                let from = self.x_of(shown, paragraph, s);
                let to =
                    self.x_of(shown, paragraph, e) + if newline { self.geometry.cell } else { 0.0 };
                let selection = Rectangle {
                    x: bounds.x + from,
                    y: bounds.y,
                    width: to - from,
                    height: bounds.height,
                };
                if let Some(clip) = selection.intersection(viewport) {
                    renderer.with_layer(clip, |renderer| {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: clip,
                                ..renderer::Quad::default()
                            },
                            self.selection.0,
                        );
                        renderer.fill_paragraph(paragraph, code.position(), self.selection.1, clip);
                        if s == 0 && self.marker != ' ' {
                            renderer.fill_text(
                                text::Text {
                                    content: self.marker.to_string(),
                                    bounds: Size::INFINITE,
                                    size: Pixels(self.geometry.size),
                                    line_height: text::LineHeight::default(),
                                    font: renderer.default_font(),
                                    align_x: text::Alignment::Default,
                                    align_y: iced::alignment::Vertical::Top,
                                    shaping: text::Shaping::default(),
                                    wrapping: text::Wrapping::None,
                                },
                                Point::new(bounds.x + self.geometry.marker_x, marker_y),
                                self.selection.1,
                                clip,
                            );
                        }
                    });
                }
            }
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
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, M: 'a> From<SelectableLine<'a, M>> for Element<'a, M> {
    fn from(w: SelectableLine<'a, M>) -> Self {
        Element::new(w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_are_one_raw_character_drawn_as_four() {
        assert_eq!(shown_index("a\tb", 1), 1);
        assert_eq!(shown_index("a\tb", 2), 5);
        assert_eq!(shown_index("a\tb", 3), 6);
        // Within a tab: the nearer of its two ends.
        assert_eq!(raw_index("a\tb", 2), 1);
        assert_eq!(raw_index("a\tb", 4), 2);
        assert_eq!(raw_index("a\tb", 5), 2);
        assert_eq!(raw_index("a\tb", 9), 3, "past the end: the end");
        // A final `\\r` is not drawn.
        assert_eq!(shown_index("ab\r", 3), 2);
    }
}
