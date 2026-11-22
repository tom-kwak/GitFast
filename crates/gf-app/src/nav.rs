//! Keyboard navigation between and inside panels.
//!
//! The app keeps one active [`Pane`]. While no text field has focus, the
//! arrow keys and friends go to that pane: each feature turns a [`Nav`]
//! into its own message (select the next commit, stage a file, ...) plus a
//! scroll that keeps the selection in view. Keys a pane does not use fall
//! back to the app (←/→ move between panes).

use iced::Task;
use iced::widget::operation;
use iced::widget::scrollable::AbsoluteOffset;

use crate::widgets::virtual_list::ScrollPos;

/// Panels that take keyboard focus, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    Sidebar,
    #[default]
    History,
    /// Files of the commit shown in the history mode.
    Files,
    /// Working tree list.
    Changes,
    /// The diff (or blame / conflict view) under the list.
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Left,
    Right,
    Enter,
    Space,
    Tab,
    /// A plain letter or symbol (by physical key, so it works with the
    /// Korean input method).
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nav {
    pub key: Key,
    pub shift: bool,
}

impl Nav {
    #[cfg(test)]
    pub fn plain(key: Key) -> Self {
        Self { key, shift: false }
    }
}

/// What a pane did with a key: a message for itself and/or a scroll.
pub struct Moved<M> {
    pub message: Option<M>,
    pub task: Task<M>,
}

impl<M> Moved<M> {
    /// The key was used but nothing needs to happen (e.g. ↑ on the first row).
    pub fn stay() -> Self {
        Self {
            message: None,
            task: Task::none(),
        }
    }

    pub fn message(message: M) -> Self {
        Self {
            message: Some(message),
            task: Task::none(),
        }
    }

    pub fn with_task(mut self, task: Task<M>) -> Self {
        self.task = task;
        self
    }

    pub fn map<N>(self, f: impl Fn(M) -> N + Send + Clone + 'static) -> Moved<N>
    where
        M: Send + 'static,
        N: Send + 'static,
    {
        Moved {
            message: self.message.map(f.clone()),
            task: self.task.map(f),
        }
    }
}

/// Index after a movement key in a list of `len` rows; `page` rows fit on
/// screen. With nothing selected, ↓/PgDn/Home start at the top and
/// ↑/PgUp/End at the bottom. `None` for keys that do not move.
pub fn step(current: Option<usize>, len: usize, key: Key, page: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    let page = page.max(1);
    Some(match (current.map(|c| c.min(last)), key) {
        (_, Key::Home) => 0,
        (_, Key::End) => last,
        (None, Key::Down | Key::PageDown) => 0,
        (None, Key::Up | Key::PageUp) => last,
        (Some(c), Key::Up) => c.saturating_sub(1),
        (Some(c), Key::Down) => (c + 1).min(last),
        (Some(c), Key::PageUp) => c.saturating_sub(page),
        (Some(c), Key::PageDown) => (c + page).min(last),
        _ => return None,
    })
}

/// Like [`step`], but lands only on rows where `stop` holds: it keeps going
/// the way the key moves and falls back the other way at the list's end.
/// Stays on `current` when no other row can be a stop.
pub fn step_to(
    current: Option<usize>,
    len: usize,
    key: Key,
    page: usize,
    stop: impl Fn(usize) -> bool,
) -> Option<usize> {
    let target = step(current, len, key, page)?;
    let forward = match (current, key) {
        (_, Key::Home) => true,
        (_, Key::End) => false,
        (Some(c), _) => target >= c,
        (None, _) => matches!(key, Key::Down | Key::PageDown),
    };
    let ahead = |from: usize| (from..len).find(|&i| stop(i));
    let behind = |from: usize| (0..=from).rev().find(|&i| stop(i));
    let found = if forward {
        ahead(target).or_else(|| behind(target))
    } else {
        behind(target).or_else(|| ahead(target))
    };
    found.or(current)
}

/// Rows that fit in the list's viewport.
pub fn page(pos: ScrollPos, row_height: f32) -> usize {
    if row_height <= 0.0 || pos.height <= 0.0 {
        return 10;
    }
    // Truncation is intended: whole rows.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rows = (pos.height / row_height) as usize;
    rows.saturating_sub(1).max(1)
}

/// Offset that brings row `index` into view, or `None` if it is visible.
pub fn reveal_offset(pos: ScrollPos, index: usize, row_height: f32) -> Option<f32> {
    #[allow(clippy::cast_precision_loss)]
    let top = index as f32 * row_height;
    let bottom = top + row_height;
    if top < pos.offset {
        Some(top)
    } else if pos.height > 0.0 && bottom > pos.offset + pos.height {
        Some(bottom - pos.height)
    } else {
        None
    }
}

/// Scrolls list `id` so row `index` is visible.
pub fn reveal<M: Send + 'static>(
    id: &'static str,
    pos: ScrollPos,
    index: usize,
    row_height: f32,
) -> Task<M> {
    match reveal_offset(pos, index, row_height) {
        Some(y) => scroll_y(id, y),
        None => Task::none(),
    }
}

/// Scrolls list `id` vertically to `y`, keeping the sideways position.
pub fn scroll_y<M: Send + 'static>(id: &'static str, y: f32) -> Task<M> {
    operation::scroll_to(
        id,
        AbsoluteOffset {
            x: None,
            y: Some(y.max(0.0)),
        },
    )
}

/// Scrolls list `id` vertically by `dy`.
pub fn scroll_by<M: Send + 'static>(id: &'static str, dy: f32) -> Task<M> {
    operation::scroll_by(id, AbsoluteOffset { x: 0.0, y: dy })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_to_skips_rows_that_are_not_stops() {
        // Rows 0 and 3 are open folders; the rest are files.
        let stop = |i: usize| i != 0 && i != 3;
        assert_eq!(step_to(Some(2), 6, Key::Down, 3, stop), Some(4));
        assert_eq!(step_to(Some(4), 6, Key::Up, 3, stop), Some(2));
        assert_eq!(step_to(Some(1), 6, Key::Up, 3, stop), Some(1), "stays");
        assert_eq!(step_to(None, 6, Key::Down, 3, stop), Some(1));
        assert_eq!(step_to(Some(5), 6, Key::Home, 3, stop), Some(1));
        assert_eq!(step_to(Some(1), 6, Key::PageDown, 2, stop), Some(4));
        assert_eq!(step_to(Some(0), 6, Key::Down, 3, stop), Some(1));
        assert_eq!(step_to(Some(0), 1, Key::Down, 3, |_| false), Some(0));
        assert_eq!(step_to(Some(1), 6, Key::Char('x'), 3, stop), None);
    }

    #[test]
    fn steps_clamp_and_start_at_the_ends() {
        assert_eq!(step(None, 5, Key::Down, 3), Some(0));
        assert_eq!(step(None, 5, Key::Up, 3), Some(4));
        assert_eq!(step(Some(0), 5, Key::Up, 3), Some(0));
        assert_eq!(step(Some(4), 5, Key::Down, 3), Some(4));
        assert_eq!(step(Some(1), 5, Key::PageDown, 3), Some(4));
        assert_eq!(step(Some(4), 5, Key::PageUp, 3), Some(1));
        assert_eq!(step(Some(2), 5, Key::End, 3), Some(4));
        assert_eq!(step(Some(9), 5, Key::Up, 3), Some(3), "stale index clamped");
        assert_eq!(step(Some(1), 5, Key::Enter, 3), None);
        assert_eq!(step(None, 0, Key::Down, 3), None);
    }

    #[test]
    fn reveals_rows_outside_the_viewport() {
        let pos = ScrollPos {
            offset: 100.0,
            height: 50.0,
            ..ScrollPos::default()
        };
        // Rows of 10px: 10..15 are visible.
        assert_eq!(reveal_offset(pos, 12, 10.0), None);
        assert_eq!(reveal_offset(pos, 5, 10.0), Some(50.0));
        assert_eq!(reveal_offset(pos, 20, 10.0), Some(160.0));
        assert_eq!(page(pos, 10.0), 4);
    }
}
