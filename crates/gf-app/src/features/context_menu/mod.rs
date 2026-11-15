//! A pop-up action menu (right-click or an `actions ▾` button), shared by
//! any feature. Features describe the entries with their own messages; the
//! app maps them and shows the menu where the mouse was pressed.
//!
//! Keyboard: the letter shown before an entry runs it, ↑/↓ + Enter choose,
//! Esc or a click outside closes.

use iced::widget::text::Wrapping;
use iced::widget::{Column, Space, button, container, mouse_area, responsive, row, stack};
use iced::{Color, Element, Length, Padding, Point};

use crate::theme::{Tokens, style};
use crate::widgets::virtual_list::columns;
use crate::widgets::{colored, dim};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    /// Rewrites history or changes many things: drawn in the warning color.
    Caution,
}

#[derive(Debug, Clone)]
pub enum Entry<M> {
    Item {
        key: char,
        label: String,
        hint: String,
        tone: Tone,
        on: M,
    },
    Separator,
}

impl<M> Entry<M> {
    pub fn item(key: char, label: impl Into<String>, hint: impl Into<String>, on: M) -> Self {
        Self::Item {
            key,
            label: label.into(),
            hint: hint.into(),
            tone: Tone::Normal,
            on,
        }
    }

    pub fn caution(key: char, label: impl Into<String>, hint: impl Into<String>, on: M) -> Self {
        match Self::item(key, label, hint, on) {
            Self::Item {
                key,
                label,
                hint,
                on,
                ..
            } => Self::Item {
                key,
                label,
                hint,
                tone: Tone::Caution,
                on,
            },
            sep => sep,
        }
    }
}

/// What a feature asks to show.
#[derive(Debug, Clone)]
pub struct Menu<M> {
    pub title: String,
    pub entries: Vec<Entry<M>>,
}

impl<M> Menu<M> {
    pub fn map<N>(self, f: impl Fn(M) -> N) -> Menu<N> {
        Menu {
            title: self.title,
            entries: self
                .entries
                .into_iter()
                .map(|e| match e {
                    Entry::Item {
                        key,
                        label,
                        hint,
                        tone,
                        on,
                    } => Entry::Item {
                        key,
                        label,
                        hint,
                        tone,
                        on: f(on),
                    },
                    Entry::Separator => Entry::Separator,
                })
                .collect(),
        }
    }
}

/// Keys the open menu reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    Char(char),
}

/// Events from the menu's own view.
#[derive(Debug, Clone, Copy)]
pub enum Message {
    Pick(usize),
    Hover(usize),
    Close,
}

/// An open menu.
#[derive(Debug, Clone)]
pub struct State<M> {
    /// Where the mouse was pressed; `None` = opened from the keyboard,
    /// shown centered.
    at: Option<Point>,
    menu: Menu<M>,
    /// Entry index chosen with the keyboard (or last hovered).
    active: Option<usize>,
}

/// What happened: run a message, close, or nothing yet.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome<M> {
    Run(M),
    Close,
    None,
}

impl<M: Clone> State<M> {
    pub fn open(at: Point, menu: Menu<M>) -> Self {
        Self {
            at: Some(at),
            menu,
            active: None,
        }
    }

    /// Opened from the keyboard: centered, first item highlighted.
    pub fn centered(menu: Menu<M>) -> Self {
        let mut s = Self {
            at: None,
            menu,
            active: None,
        };
        let first = s.items().next();
        s.active = first;
        s
    }

    fn items(&self) -> impl Iterator<Item = usize> + '_ {
        self.menu
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, Entry::Item { .. }))
            .map(|(i, _)| i)
    }

    fn run(&self, i: usize) -> Outcome<M> {
        match self.menu.entries.get(i) {
            Some(Entry::Item { on, .. }) => Outcome::Run(on.clone()),
            _ => Outcome::None,
        }
    }

    pub fn update(&mut self, message: Message) -> Outcome<M> {
        match message {
            Message::Pick(i) => self.run(i),
            Message::Hover(i) => {
                self.active = Some(i);
                Outcome::None
            }
            Message::Close => Outcome::Close,
        }
    }

    pub fn key(&mut self, key: Key) -> Outcome<M> {
        let items: Vec<usize> = self.items().collect();
        let pos = self.active.and_then(|a| items.iter().position(|i| *i == a));
        match key {
            Key::Down | Key::Up if items.is_empty() => Outcome::None,
            Key::Down => {
                let next = pos.map_or(0, |p| (p + 1) % items.len());
                self.active = Some(items[next]);
                Outcome::None
            }
            Key::Up => {
                let prev = pos.map_or(items.len() - 1, |p| (p + items.len() - 1) % items.len());
                self.active = Some(items[prev]);
                Outcome::None
            }
            Key::Enter => self.active.map_or(Outcome::None, |i| self.run(i)),
            Key::Char(c) => {
                let c = c.to_ascii_lowercase();
                let hit = self.menu.entries.iter().position(
                    |e| matches!(e, Entry::Item { key, .. } if key.to_ascii_lowercase() == c),
                );
                hit.map_or(Outcome::None, |i| self.run(i))
            }
        }
    }

    /// Width and height the menu takes.
    fn extent(&self, t: &Tokens) -> (f32, f32) {
        let row_h = (t.font_size * 1.6).round();
        let label_cols = self
            .menu
            .entries
            .iter()
            .map(|e| match e {
                // Wide characters (Hangul, CJK) take two columns.
                Entry::Item { label, .. } => columns(label) + 3,
                Entry::Separator => 0,
            })
            .chain(std::iter::once(columns(&self.menu.title)))
            .max()
            .unwrap_or(10);
        let height = self
            .menu
            .entries
            .iter()
            .map(|e| match e {
                Entry::Item { .. } => row_h,
                Entry::Separator => SEPARATOR_H,
            })
            .sum::<f32>()
            + row_h
            + 8.0;
        (t.cols(label_cols) + 24.0, height)
    }

    fn menu_box<'a>(&'a self, t: &'a Tokens, width: f32) -> Element<'a, Message> {
        let row_h = (t.font_size * 1.6).round();
        let mut list = Column::new()
            .push(
                container(dim(t, self.menu.title.clone()).wrapping(Wrapping::None))
                    .height(Length::Fixed(row_h))
                    .padding(Padding::from([2, 8])),
            )
            .width(Length::Fixed(width));
        for (i, e) in self.menu.entries.iter().enumerate() {
            list = list.push(match e {
                Entry::Separator => container(
                    container(Space::new().width(Length::Fill).height(1.0)).style(move |_| {
                        container::Style {
                            background: Some(t.dim.into()),
                            ..container::Style::default()
                        }
                    }),
                )
                .height(Length::Fixed(SEPARATOR_H))
                .padding(Padding::from([4, 6]))
                .into(),
                Entry::Item {
                    key,
                    label,
                    tone,
                    hint,
                    ..
                } => {
                    let color = match tone {
                        Tone::Normal => t.fg,
                        Tone::Caution => t.warning(),
                    };
                    let line = row![
                        colored(t, key.to_string(), t.accent()),
                        colored(t, label.clone(), color).wrapping(Wrapping::None),
                    ]
                    .spacing(10);
                    let item = mouse_area(
                        button(line)
                            .width(Length::Fill)
                            .height(Length::Fixed(row_h))
                            .padding(Padding::from([1, 8]))
                            .style(style::row_button(t, self.active == Some(i)))
                            .on_press(Message::Pick(i)),
                    )
                    .on_enter(Message::Hover(i));
                    crate::widgets::tip_right(t, item, hint.clone())
                }
            });
        }
        container(list)
            .padding(Padding::from([2, 0]))
            .style(style::tooltip(t))
            .into()
    }

    /// The menu drawn at the press position (kept inside the window) over
    /// a transparent layer that closes it when clicked.
    pub fn view<'a>(&'a self, t: &'a Tokens) -> Element<'a, Message> {
        let (width, height) = self.extent(t);
        let at = self.at;
        let placed = responsive(move |size| {
            let (x, y) = match at {
                // Keep the whole menu on screen: shift left, or open upwards.
                Some(at) => (
                    at.x.min(size.width - width - 4.0).max(0.0),
                    if at.y + height > size.height {
                        (at.y - height).max(0.0)
                    } else {
                        at.y
                    },
                ),
                None => (
                    ((size.width - width) / 2.0).max(0.0),
                    ((size.height - height) / 3.0).max(0.0),
                ),
            };
            container(self.menu_box(t, width))
                .padding(Padding {
                    top: y,
                    left: x,
                    right: 0.0,
                    bottom: 0.0,
                })
                .into()
        });
        let backdrop = mouse_area(
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Color::TRANSPARENT.into()),
                    ..container::Style::default()
                }),
        )
        .on_press(Message::Close)
        .on_right_press(Message::Close);
        stack![backdrop, placed].into()
    }
}

const SEPARATOR_H: f32 = 9.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> State<u8> {
        State::open(
            Point::ORIGIN,
            Menu {
                title: "t".into(),
                entries: vec![
                    Entry::item('b', "branch", "", 1),
                    Entry::Separator,
                    Entry::caution('r', "revert", "", 2),
                    Entry::item('y', "copy", "", 3),
                ],
            },
        )
    }

    #[test]
    fn letters_run_their_entry() {
        let mut m = menu();
        assert_eq!(m.key(Key::Char('R')), Outcome::Run(2));
        assert_eq!(m.key(Key::Char('z')), Outcome::None);
    }

    #[test]
    fn arrows_skip_separators_and_wrap() {
        let mut m = menu();
        assert_eq!(m.key(Key::Enter), Outcome::None, "nothing chosen yet");
        let _ = m.key(Key::Down);
        let _ = m.key(Key::Down);
        assert_eq!(m.key(Key::Enter), Outcome::Run(2));
        let _ = m.key(Key::Down);
        let _ = m.key(Key::Down);
        assert_eq!(m.key(Key::Enter), Outcome::Run(1), "wraps to the top");
        let _ = m.key(Key::Up);
        assert_eq!(m.key(Key::Enter), Outcome::Run(3));
    }

    #[test]
    fn maps_messages() {
        let m = menu().menu.map(|n| u32::from(n) * 10);
        assert!(matches!(
            m.entries[2],
            Entry::Item {
                on: 20,
                tone: Tone::Caution,
                ..
            }
        ));
    }
}
