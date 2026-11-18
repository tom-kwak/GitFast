//! Terminal-style design tokens (PLAN.md §4-1).
//!
//! Every color and size used by the UI comes from [`Tokens`]; widgets never
//! hard-code colors. A theme is a terminal color scheme: background,
//! foreground and the 8 ANSI colors, plus a few derived surface colors.

mod presets;
pub mod style;

use iced::Color;
use iced::theme::Palette;

pub use presets::{PRESET_NAMES, preset};

/// The 8 standard ANSI colors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ansi {
    pub black: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub magenta: Color,
    pub cyan: Color,
    pub white: Color,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    pub name: &'static str,
    pub dark: bool,
    /// Window background.
    pub bg: Color,
    /// Bars (tab bar, status line) — slightly offset from `bg`.
    pub surface: Color,
    pub fg: Color,
    /// Secondary text: hints, dates, separators.
    pub dim: Color,
    /// 1px panel separators.
    pub border: Color,
    /// Selected row / hovered button background.
    pub selection: Color,
    /// Background and foreground of selected text in the diff.
    pub diff_selection: (Color, Color),
    pub ansi: Ansi,
    /// Base monospace font size.
    pub font_size: f32,
}

impl Tokens {
    pub fn with_font_size(mut self, size: u16) -> Self {
        self.font_size = f32::from(size.clamp(9, 32));
        self
    }

    // Semantic roles, so the meaning of a color is decided here only.
    pub fn added(&self) -> Color {
        self.ansi.green
    }
    pub fn removed(&self) -> Color {
        self.ansi.red
    }
    pub fn branch(&self) -> Color {
        self.ansi.cyan
    }
    pub fn remote(&self) -> Color {
        self.ansi.blue
    }
    pub fn tag(&self) -> Color {
        self.ansi.yellow
    }
    pub fn head(&self) -> Color {
        self.ansi.magenta
    }
    pub fn error(&self) -> Color {
        self.ansi.red
    }
    pub fn warning(&self) -> Color {
        self.ansi.yellow
    }
    pub fn success(&self) -> Color {
        self.ansi.green
    }
    pub fn accent(&self) -> Color {
        self.ansi.cyan
    }

    /// Color for a syntax highlighting tone.
    pub fn tone(&self, tone: crate::highlight::Tone) -> Color {
        use crate::highlight::Tone;
        let a = &self.ansi;
        match tone {
            Tone::Plain | Tone::Variable => self.fg,
            Tone::Comment => self.dim,
            Tone::String => a.green,
            Tone::Number | Tone::Attribute => a.yellow,
            Tone::Keyword => a.magenta,
            Tone::Function => a.blue,
            Tone::Type => a.cyan,
            Tone::Tag | Tone::Invalid => a.red,
        }
    }

    /// Color of graph lane `i` (cycles through `LANE_COLORS` hues).
    pub fn lane(&self, i: usize) -> Color {
        let a = &self.ansi;
        [a.cyan, a.magenta, a.yellow, a.green, a.blue, a.red][i % 6]
    }

    /// Width of one monospace column at the base font size.
    pub fn char_width(&self) -> f32 {
        self.font_size * 0.61
    }

    /// Width of `n` monospace columns.
    pub fn cols(&self, n: usize) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let n = n as f32;
        n * self.char_width()
    }

    /// Font size for secondary text.
    pub fn small(&self) -> f32 {
        (self.font_size - 1.0).max(9.0)
    }

    /// The iced theme matching these tokens (used for widget defaults such as
    /// scrollbars and text selection).
    pub fn iced_theme(&self) -> iced::Theme {
        iced::Theme::custom(
            self.name,
            Palette {
                background: self.bg,
                text: self.fg,
                primary: self.accent(),
                success: self.success(),
                warning: self.warning(),
                danger: self.error(),
            },
        )
    }
}
