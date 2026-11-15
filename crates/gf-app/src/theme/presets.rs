//! Built-in color schemes. Add a scheme by adding a function and a line in
//! [`preset`]; names are what the settings file and `:theme <name>` use.

use iced::Color;

use super::{Ansi, Tokens};

pub const PRESET_NAMES: &[&str] = &["gf-dark", "gf-light", "dracula"];

const DEFAULT_FONT_SIZE: f32 = 13.0;

const fn rgb(hex: u32) -> Color {
    Color::from_rgb8(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

/// Looks up a preset by name; unknown names fall back to `gf-dark`.
pub fn preset(name: &str) -> Tokens {
    match name {
        "gf-light" => gf_light(),
        "dracula" => dracula(),
        _ => gf_dark(),
    }
}

fn gf_dark() -> Tokens {
    Tokens {
        name: "gf-dark",
        dark: true,
        bg: rgb(0x0f1115),
        surface: rgb(0x181b21),
        fg: rgb(0xd7dae0),
        dim: rgb(0x6b7280),
        border: rgb(0x2a2f38),
        selection: rgb(0x26303d),
        ansi: Ansi {
            black: rgb(0x1d2026),
            red: rgb(0xf26d6d),
            green: rgb(0x7fd18b),
            yellow: rgb(0xe8c06a),
            blue: rgb(0x6ea8fe),
            magenta: rgb(0xd38cf2),
            cyan: rgb(0x5fd4d4),
            white: rgb(0xe6e6e6),
        },
        font_size: DEFAULT_FONT_SIZE,
    }
}

fn gf_light() -> Tokens {
    Tokens {
        name: "gf-light",
        dark: false,
        bg: rgb(0xfbfbf8),
        surface: rgb(0xefefe9),
        fg: rgb(0x24292f),
        dim: rgb(0x7a7f87),
        border: rgb(0xd5d5cf),
        selection: rgb(0xdde6f0),
        ansi: Ansi {
            black: rgb(0x24292f),
            red: rgb(0xc4302b),
            green: rgb(0x2b8a3e),
            yellow: rgb(0x9a6700),
            blue: rgb(0x0b5cad),
            magenta: rgb(0x8f3fb8),
            cyan: rgb(0x0f7b8a),
            white: rgb(0xffffff),
        },
        font_size: DEFAULT_FONT_SIZE,
    }
}

fn dracula() -> Tokens {
    Tokens {
        name: "dracula",
        dark: true,
        bg: rgb(0x282a36),
        surface: rgb(0x21222c),
        fg: rgb(0xf8f8f2),
        dim: rgb(0x6272a4),
        border: rgb(0x44475a),
        selection: rgb(0x44475a),
        ansi: Ansi {
            black: rgb(0x21222c),
            red: rgb(0xff5555),
            green: rgb(0x50fa7b),
            yellow: rgb(0xf1fa8c),
            blue: rgb(0xbd93f9),
            magenta: rgb(0xff79c6),
            cyan: rgb(0x8be9fd),
            white: rgb(0xf8f8f2),
        },
        font_size: DEFAULT_FONT_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_preset_resolves_to_itself() {
        for name in PRESET_NAMES {
            assert_eq!(preset(name).name, *name);
        }
        assert_eq!(preset("nope").name, "gf-dark");
    }
}
