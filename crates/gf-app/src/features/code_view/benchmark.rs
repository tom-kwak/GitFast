//! Headless checks of the file viewport and scrolling performance.

use std::time::Instant;

use iced::advanced::clipboard;
use iced::advanced::renderer::{self, Headless};
use iced::advanced::widget::operation::scrollable::{Scrollable, scroll_to};
use iced::advanced::widget::{Id, Operation};
use iced::futures::{StreamExt, executor::block_on};
use iced::widget::scrollable::AbsoluteOffset;
use iced::{Event, Font, Pixels, Rectangle, Renderer, Size, Theme, Vector, mouse};
use iced_test::runtime::{Action, UserInterface, task};

use super::*;

#[test]
fn file_viewport_grows_with_the_window_in_both_modes() {
    let tokens = crate::theme::preset("gf-dark");
    let mut renderer = headless_renderer();
    for split in [false, true] {
        let state = prepared_view("a line\n", split);
        let ids: &[_] = if split { &COMPARE_LISTS } else { &[LIST_ID] };
        let mut viewport = |size| {
            let mut ui = UserInterface::build(
                state.view(Cx {
                    tokens: &tokens,
                    snapshot: None,
                    all_branches: false,
                    file_tree: false,
                    staging: false,
                }),
                size,
                Default::default(),
                &mut renderer,
            );
            let mut offsets = ScrollOffsets::new(ids);
            ui.operate(&renderer, &mut offsets);
            let mut panes = offsets.bounds[..ids.len()].iter().flatten();
            let first = panes.next().expect("file viewport");
            Size::new(
                first.width + panes.map(|pane| pane.width).sum::<f32>(),
                first.height,
            )
        };
        let base = viewport(Size::new(1280.0, 820.0));
        let expanded = viewport(Size::new(1920.0, 1200.0));
        assert!(expanded.width - base.width > 590.0);
        assert!(expanded.height - base.height > 350.0);
    }
}

#[test]
#[ignore = "manual performance benchmark"]
fn bench_large_file_scroll() {
    let body = "abcdefghij0123456789".repeat(600);
    let long_lines = (0..144)
        .map(|i| format!("{i:05} {body}\n"))
        .collect::<String>();
    let many_lines = (0..24_000)
        .map(|i| format!("{i:05} abcdefghij0123456789 abcdefghij0123456789\n"))
        .collect::<String>();
    let tokens = crate::theme::preset("gf-dark");
    let mut renderer = headless_renderer();
    for (name, file) in [("long lines", long_lines), ("many lines", many_lines)] {
        for split in [false, true] {
            let start = Instant::now();
            let mut state = prepared_view(&file, split);
            let prep = start.elapsed();
            let mut cache = Default::default();
            let mut frames = Vec::new();
            for frame in 0_u16..76 {
                let offset = f32::from(frame) * row_height(&tokens);
                let scroll = ScrollPos {
                    offset,
                    height: 710.0,
                    content: 0.0,
                };
                let event = if split {
                    Message::CompareScrolled(BEFORE_LIST, scroll)
                } else {
                    Message::Scrolled(scroll)
                };
                let start = Instant::now();
                let _ = state.update(event);
                let mut ui = UserInterface::build(
                    state.view(Cx {
                        tokens: &tokens,
                        snapshot: None,
                        all_branches: false,
                        file_tree: false,
                        staging: false,
                    }),
                    Size::new(1280.0, 820.0),
                    cache,
                    &mut renderer,
                );
                let ids: &[_] = if split { &COMPARE_LISTS } else { &[LIST_ID] };
                for &id in ids {
                    ui.operate(
                        &renderer,
                        &mut scroll_to::<()>(
                            id.into(),
                            AbsoluteOffset {
                                x: None,
                                y: Some(offset),
                            },
                        ),
                    );
                }
                ui.draw(
                    &mut renderer,
                    &Theme::Dark,
                    &renderer::Style::default(),
                    mouse::Cursor::Unavailable,
                );
                cache = ui.into_cache();
                if frame >= 12 {
                    frames.push(start.elapsed().as_micros());
                }
            }
            eprintln!(
                "{name} · split={split} · prep={} us · {}",
                prep.as_micros(),
                timings(&mut frames),
            );
        }
    }
}

#[test]
#[ignore = "manual scroll interaction benchmark"]
fn bench_long_file_wheel_actions() {
    let body = "abcdefghij0123456789".repeat(600);
    let file = (0..144)
        .map(|i| format!("{i:05} {body}\n"))
        .collect::<String>();
    let tokens = crate::theme::preset("gf-dark");
    let mut renderer = headless_renderer();
    for (label, split, source, movement) in [
        ("single vertical", false, 0, Vector::new(0.0, 60.0)),
        ("single horizontal", false, 0, Vector::new(60.0, 0.0)),
        ("split left vertical", true, 0, Vector::new(0.0, 60.0)),
        ("split right vertical", true, 2, Vector::new(0.0, 60.0)),
        ("split left horizontal", true, 0, Vector::new(60.0, 0.0)),
        ("split right horizontal", true, 2, Vector::new(60.0, 0.0)),
    ] {
        let ids: &[_] = if split { &COMPARE_LISTS } else { &[LIST_ID] };
        let mut state = prepared_view(&file, split);
        let mut ui = UserInterface::build(
            state.view(Cx {
                tokens: &tokens,
                snapshot: None,
                all_branches: false,
                file_tree: false,
                staging: false,
            }),
            Size::new(1280.0, 820.0),
            Default::default(),
            &mut renderer,
        );
        let mut offsets = ScrollOffsets::new(ids);
        ui.operate(&renderer, &mut offsets);
        let bounds = offsets.bounds[source].expect("scrollable viewport");
        let cursor = mouse::Cursor::Available(bounds.center());
        let mut frames = Vec::new();
        // Reverse direction halfway, as when scrolling back to a line just passed.
        for frame in 0..76 {
            let direction = if frame < 38 { 1.0 } else { -1.0 };
            let delta = movement * direction;
            let previous = offsets.positions;
            let event = Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Pixels {
                    x: -delta.x,
                    y: -delta.y,
                },
            });
            let start = Instant::now();
            let mut messages = Vec::new();
            let (_, statuses) = ui.update(
                &[event],
                cursor,
                &mut renderer,
                &mut clipboard::Null,
                &mut messages,
            );
            assert!(matches!(
                statuses.as_slice(),
                [iced::event::Status::Captured]
            ));
            assert!(
                !messages.is_empty(),
                "wheel event reports its scroll offset"
            );
            let cache = ui.into_cache();
            let tasks: Vec<_> = messages
                .into_iter()
                .map(|message| state.update(message).task)
                .collect();
            ui = UserInterface::build(
                state.view(Cx {
                    tokens: &tokens,
                    snapshot: None,
                    all_branches: false,
                    file_tree: false,
                    staging: false,
                }),
                Size::new(1280.0, 820.0),
                cache,
                &mut renderer,
            );
            if let Some(mut stream) = task::into_stream(Task::batch(tasks)) {
                block_on(async {
                    while let Some(action) = stream.next().await {
                        let Action::Widget(mut operation) = action else {
                            panic!("wheel tasks only move scroll widgets");
                        };
                        ui.operate(&renderer, operation.as_mut());
                    }
                });
            }
            ui.draw(
                &mut renderer,
                &Theme::Dark,
                &renderer::Style::default(),
                cursor,
            );
            if frame >= 12 {
                frames.push(start.elapsed().as_micros());
            }
            ui.operate(&renderer, &mut offsets);
            for (pane, actual) in offsets.positions[..ids.len()].iter().enumerate() {
                let expected_x = previous[pane].x + if pane == source { delta.x } else { 0.0 };
                let expected_y = previous[pane].y + delta.y;
                assert!(
                    (actual.x - expected_x).abs() < 0.5,
                    "{label} · pane {pane} · horizontal offset {actual:?} != {expected_x}"
                );
                assert!(
                    (actual.y - expected_y).abs() < 0.5,
                    "{label} · pane {pane} · vertical offset {actual:?} != {expected_y}"
                );
            }
        }
        eprintln!("{label} · {}", timings(&mut frames));
    }
}

fn headless_renderer() -> Renderer {
    block_on(Renderer::new(
        Font::MONOSPACE,
        Pixels(14.0),
        Some("tiny-skia"),
    ))
    .expect("headless renderer")
}

fn prepared_view(file: &str, split: bool) -> State {
    let path = Path::new("sample.txt");
    let mut state = tests::state();
    state.path = path.into();
    let mut document = Document::from_bytes(path, file.as_bytes().to_vec());
    if split {
        let Document::Text(before) = Document::from_bytes(path, file.as_bytes().to_vec()) else {
            panic!("expected text");
        };
        let diff = FileDiff {
            old_path: Some(path.into()),
            new_path: Some(path.into()),
            change: FileChange::Modified,
            old_mode: None,
            new_mode: None,
            is_binary: false,
            hunks: Vec::new(),
        };
        document.show_changes(&diff, Some(before));
        state.comparison_labels = Some(["Before", "After"]);
    }
    let _ = state.update(Message::Loaded(state.request, Ok(Arc::new(document))));
    let _ = state.update(Message::SetSplit(split));
    assert_eq!(state.split, split);
    state
}

fn timings(frames: &mut [u128]) -> String {
    frames.sort_unstable();
    let median = frames[frames.len() / 2];
    let p95 = frames[(frames.len() * 95).div_ceil(100) - 1];
    let max = frames.last().expect("timed frames");
    format!("median={median} us · p95={p95} us · max={max} us")
}

struct ScrollOffsets {
    ids: Vec<Id>,
    bounds: [Option<Rectangle>; 3],
    positions: [Vector; 3],
}

impl ScrollOffsets {
    fn new(ids: &[&'static str]) -> Self {
        Self {
            ids: ids.iter().copied().map(Id::from).collect(),
            bounds: [None; 3],
            positions: [Vector::ZERO; 3],
        }
    }
}

impl Operation for ScrollOffsets {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn Scrollable,
    ) {
        if let Some(index) = self.ids.iter().position(|target| Some(target) == id) {
            self.bounds[index] = Some(bounds);
            self.positions[index] = translation;
        }
    }
}
