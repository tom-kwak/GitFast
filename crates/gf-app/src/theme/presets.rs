//! Built-in color schemes. Add a scheme by adding a function and registering it in
//! [`PRESET_NAMES`] and [`preset`]; names are what the settings file and `:theme <name>` use.

use iced::Color;

use super::{Ansi, Tokens};

pub const PRESET_NAMES: &[&str] = &["gf-dark", "gf-light", "dracula", "cd-dark", "cd-light"];

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
        "cd-dark" | "claude-dark" => claude_dark(),
        "cd-light" | "claude-light" => claude_light(),
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
        accent: rgb(0x5fd4d4),
        dim: rgb(0x6b7280),
        border: rgb(0x2a2f38),
        selection: rgb(0x26303d),
        diff_selection: (rgb(0x315b9c), rgb(0xffffff)),
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
        accent: rgb(0x0f7b8a),
        dim: rgb(0x7a7f87),
        border: rgb(0xd5d5cf),
        selection: rgb(0xdde6f0),
        diff_selection: (rgb(0x9bc3ff), rgb(0x102544)),
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
        accent: rgb(0x8be9fd),
        dim: rgb(0x6272a4),
        border: rgb(0x44475a),
        selection: rgb(0x44475a),
        diff_selection: (rgb(0x6952a3), rgb(0xf8f8f2)),
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

fn claude_dark() -> Tokens {
    Tokens {
        name: "cd-dark",
        dark: true,
        bg: rgb(0x201f1c),
        surface: rgb(0x2a2925),
        fg: rgb(0xfaf9f5),
        accent: rgb(0xd97757),
        dim: rgb(0xb0aea5),
        border: rgb(0x504b42),
        selection: rgb(0x49352c),
        diff_selection: (rgb(0xd97757), rgb(0x141413)),
        ansi: Ansi {
            black: rgb(0x292824),
            red: rgb(0xe48880),
            green: rgb(0xa7b887),
            yellow: rgb(0xdfbc78),
            blue: rgb(0x83abd1),
            magenta: rgb(0xc79cb6),
            cyan: rgb(0x8bb7ac),
            white: rgb(0xe8e6dc),
        },
        font_size: DEFAULT_FONT_SIZE,
    }
}

fn claude_light() -> Tokens {
    Tokens {
        name: "cd-light",
        dark: false,
        bg: rgb(0xfaf9f5),
        surface: rgb(0xf0eee6),
        fg: rgb(0x292722),
        accent: rgb(0x98472e),
        dim: rgb(0x706d62),
        border: rgb(0xd5cec0),
        selection: rgb(0xeedccd),
        diff_selection: (rgb(0x9c5037), rgb(0xfaf9f5)),
        ansi: Ansi {
            black: rgb(0x292722),
            red: rgb(0xa14043),
            green: rgb(0x536b3e),
            yellow: rgb(0x805c21),
            blue: rgb(0x476c91),
            magenta: rgb(0x85576e),
            cyan: rgb(0x446d64),
            white: rgb(0xfaf9f5),
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
        assert_eq!(preset("claude-dark").name, "cd-dark");
        assert_eq!(preset("claude-light").name, "cd-light");
        assert_eq!(preset("nope").name, "gf-dark");
    }

    #[test]
    fn diff_selection_text_contrasts_with_its_background_in_every_theme() {
        fn luminance(c: Color) -> f32 {
            let linear = |v: f32| {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
        }

        for name in PRESET_NAMES {
            let (bg, fg) = preset(name).diff_selection;
            let (bg, fg) = (luminance(bg), luminance(fg));
            assert!((bg.max(fg) + 0.05) / (bg.min(fg) + 0.05) >= 4.5, "{name}");
        }
    }
}
