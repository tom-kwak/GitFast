//! A headless window for the sweep, like `iced_test::Simulator` but kept
//! warm the way the running app is.
//!
//! `Simulator` starts from nothing every time: a new renderer and an empty
//! widget tree, so every press, look for targets or frame shaped all the
//! text again. Here the renderer is one per test thread, and each [`Ui`]
//! keeps its widget state between layouts (`user_interface::Cache`), as
//! the app keeps it between frames.
//!
//! [`Ui`]: super::Ui

use std::cell::RefCell;
use std::hash::{Hash, Hasher};

use iced_test::core::renderer::Headless;
use iced_test::core::{Element, Event, Size, clipboard, mouse, renderer, widget, window};
use iced_test::renderer::Renderer;
use iced_test::runtime::{UserInterface, user_interface};

use super::{Message, WINDOW};

std::thread_local! {
    /// The thread's renderer while no window holds it.
    static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
}

fn renderer() -> Renderer {
    RENDERER.take().unwrap_or_else(|| {
        let backend = std::env::var("ICED_TEST_BACKEND").ok();
        iced::futures::executor::block_on(Renderer::new(
            crate::platform::mono_font(),
            iced_test::core::Settings::default().default_text_size,
            backend.as_deref(),
        ))
        .expect("a headless renderer")
    })
}

/// `view` laid out in a [`WINDOW`]-sized window.
pub(super) struct Sim<'a> {
    ui: Option<UserInterface<'a, Message, iced::Theme, Renderer>>,
    renderer: Option<Renderer>,
    /// Where the widget state goes back when the window is done.
    widgets: &'a RefCell<Option<user_interface::Cache>>,
    cursor: mouse::Cursor,
    messages: Vec<Message>,
}

impl<'a> Sim<'a> {
    /// `view` laid out with the widget state in `widgets` (empty: a fresh
    /// window), which gets the state back when the window is dropped.
    pub fn new(
        view: Element<'a, Message, iced::Theme, Renderer>,
        widgets: &'a RefCell<Option<user_interface::Cache>>,
    ) -> Self {
        let mut renderer = renderer();
        let cache = widgets.take().unwrap_or_default();
        let ui = UserInterface::build(view, WINDOW, cache, &mut renderer);
        Self {
            ui: Some(ui),
            renderer: Some(renderer),
            widgets,
            cursor: mouse::Cursor::Unavailable,
            messages: Vec::new(),
        }
    }

    fn parts(
        &mut self,
    ) -> (
        &mut UserInterface<'a, Message, iced::Theme, Renderer>,
        &mut Renderer,
    ) {
        (
            self.ui.as_mut().expect("laid out"),
            self.renderer.as_mut().expect("a renderer"),
        )
    }

    /// What `selector` finds in the window, if anything.
    pub fn find<S>(&mut self, selector: S) -> Option<S::Output>
    where
        S: iced_test::Selector + Send,
        S::Output: Clone + Send,
    {
        use widget::Operation;
        let mut operation = selector.find();
        let (ui, renderer) = self.parts();
        ui.operate(renderer, &mut widget::operation::black_box(&mut operation));
        match operation.finish() {
            widget::operation::Outcome::Some(found) => found,
            _ => None,
        }
    }

    /// Points the mouse at `at` (without a move event).
    pub fn point_at(&mut self, at: iced_test::core::Point) {
        self.cursor = mouse::Cursor::Available(at);
    }

    /// Feeds `events` to the window; the messages they produce are kept.
    pub fn simulate(&mut self, events: impl IntoIterator<Item = Event>) {
        let events: Vec<Event> = events.into_iter().collect();
        let cursor = self.cursor;
        let ui = self.ui.as_mut().expect("laid out");
        let renderer = self.renderer.as_mut().expect("a renderer");
        let _ = ui.update(
            &events,
            cursor,
            renderer,
            &mut clipboard::Null,
            &mut self.messages,
        );
    }

    /// A hash of the window drawn as `Simulator::snapshot` draws it.
    pub fn frame(&mut self, theme: &iced::Theme) -> u64 {
        use iced_test::core::theme::Base;
        let base = theme.base();
        let cursor = self.cursor;
        let ui = self.ui.as_mut().expect("laid out");
        let renderer = self.renderer.as_mut().expect("a renderer");
        let _ = ui.update(
            &[Event::Window(window::Event::RedrawRequested(
                iced_test::core::time::Instant::now(),
            ))],
            cursor,
            renderer,
            &mut clipboard::Null,
            &mut self.messages,
        );
        ui.draw(
            renderer,
            theme,
            &renderer::Style {
                text_color: base.text_color,
            },
            cursor,
        );
        let scale = 2.0;
        let size = Size::new(
            (WINDOW.width * scale).round() as u32,
            (WINDOW.height * scale).round() as u32,
        );
        let rgba = renderer.screenshot(size, scale, base.background_color);
        let mut hasher = std::hash::DefaultHasher::new();
        rgba.hash(&mut hasher);
        hasher.finish()
    }

    /// The messages the events produced.
    pub fn into_messages(mut self) -> std::vec::IntoIter<Message> {
        std::mem::take(&mut self.messages).into_iter()
    }
}

impl Drop for Sim<'_> {
    fn drop(&mut self) {
        if let Some(ui) = self.ui.take() {
            *self.widgets.borrow_mut() = Some(ui.into_cache());
        }
        if let Some(renderer) = self.renderer.take() {
            RENDERER.with_borrow_mut(|slot| {
                if slot.is_none() {
                    *slot = Some(renderer);
                }
            });
        }
    }
}
