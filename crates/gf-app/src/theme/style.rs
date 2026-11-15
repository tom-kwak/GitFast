//! Widget style functions built from [`Tokens`]. Square corners, 1px
//! borders, no shadows — a terminal look — while keeping clear hover and
//! pressed states so every clickable thing visibly reacts (PLAN.md §4-2).

use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Theme};

use super::Tokens;

fn square(color: Color, width: f32) -> Border {
    Border {
        color,
        width,
        radius: 0.0.into(),
    }
}

/// The window background.
pub fn app(t: &Tokens) -> impl Fn(&Theme) -> container::Style + '_ {
    move |_| container::Style {
        text_color: Some(t.fg),
        background: Some(Background::Color(t.bg)),
        ..container::Style::default()
    }
}

/// A bordered panel (sidebar, lists, command log).
pub fn panel(t: &Tokens) -> impl Fn(&Theme) -> container::Style + '_ {
    move |_| container::Style {
        text_color: Some(t.fg),
        background: Some(Background::Color(t.bg)),
        border: square(t.border, 1.0),
        ..container::Style::default()
    }
}

/// Tab bar and status line.
pub fn bar(t: &Tokens) -> impl Fn(&Theme) -> container::Style + '_ {
    move |_| container::Style {
        text_color: Some(t.fg),
        background: Some(Background::Color(t.surface)),
        ..container::Style::default()
    }
}

/// Tooltip box.
pub fn tooltip(t: &Tokens) -> impl Fn(&Theme) -> container::Style + '_ {
    move |_| container::Style {
        text_color: Some(t.fg),
        background: Some(Background::Color(t.surface)),
        border: square(t.dim, 1.0),
        ..container::Style::default()
    }
}

/// Text-only button (`[ Open ⌘O ]`): transparent until hovered.
pub fn text_button(
    t: &Tokens,
    color: Color,
) -> impl Fn(&Theme, button::Status) -> button::Style + '_ {
    move |_, status| {
        let background = match status {
            button::Status::Hovered => Some(Background::Color(t.selection)),
            button::Status::Pressed => Some(Background::Color(t.border)),
            button::Status::Active | button::Status::Disabled => None,
        };
        button::Style {
            background,
            text_color: if status == button::Status::Disabled {
                t.dim
            } else {
                color
            },
            border: square(Color::TRANSPARENT, 0.0),
            ..button::Style::default()
        }
    }
}

/// A full-width list row: highlighted when selected or hovered.
pub fn row_button(
    t: &Tokens,
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + '_ {
    move |_, status| {
        let background = match (selected, status) {
            (true, _) => Some(Background::Color(t.selection)),
            (false, button::Status::Hovered) => Some(Background::Color(t.surface)),
            (false, button::Status::Pressed) => Some(Background::Color(t.selection)),
            _ => None,
        };
        button::Style {
            background,
            text_color: t.fg,
            border: square(Color::TRANSPARENT, 0.0),
            ..button::Style::default()
        }
    }
}

/// A tab in the tab bar; the active one is drawn inverted like tmux.
pub fn tab(t: &Tokens, active: bool) -> impl Fn(&Theme, button::Status) -> button::Style + '_ {
    move |_, status| {
        let (background, text_color) = match (active, status) {
            (true, _) => (Some(Background::Color(t.accent())), t.bg),
            (false, button::Status::Hovered | button::Status::Pressed) => {
                (Some(Background::Color(t.selection)), t.fg)
            }
            (false, _) => (None, t.dim),
        };
        button::Style {
            background,
            text_color,
            border: square(Color::TRANSPARENT, 0.0),
            ..button::Style::default()
        }
    }
}

/// The `gf ❯` command line: no box, just text on the background.
pub fn prompt_input(t: &Tokens) -> impl Fn(&Theme, text_input::Status) -> text_input::Style + '_ {
    move |_, _| text_input::Style {
        background: Background::Color(t.bg),
        border: square(Color::TRANSPARENT, 0.0),
        icon: t.dim,
        placeholder: t.dim,
        value: t.fg,
        selection: t.selection,
    }
}

/// Bordered single-line input used in dialogs and filters.
pub fn field_input(t: &Tokens) -> impl Fn(&Theme, text_input::Status) -> text_input::Style + '_ {
    move |_, status| text_input::Style {
        background: Background::Color(t.bg),
        border: square(
            if matches!(status, text_input::Status::Focused { .. }) {
                t.accent()
            } else {
                t.border
            },
            1.0,
        ),
        icon: t.dim,
        placeholder: t.dim,
        value: t.fg,
        selection: t.selection,
    }
}
