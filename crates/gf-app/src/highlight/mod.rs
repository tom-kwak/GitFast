//! Syntax highlighting for diffs with TextMate grammars (the model VS Code
//! uses): bat's bundled grammars via `two-face`, plus user grammars dropped
//! into `<config>/syntaxes/` as VS Code `*.tmLanguage.json` (converted by
//! [`tmlanguage`]) or `*.sublime-syntax`. Regexes run on Oniguruma, the
//! same engine as VS Code.
//!
//! Highlighting produces [`Tone`]s (comment, string, keyword, ...) rather
//! than colors, so results don't depend on the active theme; the view maps
//! tones to the terminal palette.

pub mod tmlanguage;

use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Semantic class of a highlighted span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    Plain,
    Comment,
    String,
    Number,
    Keyword,
    Function,
    Type,
    Tag,
    Attribute,
    Variable,
    Invalid,
}

const TONES: [Tone; 11] = [
    Tone::Plain,
    Tone::Comment,
    Tone::String,
    Tone::Number,
    Tone::Keyword,
    Tone::Function,
    Tone::Type,
    Tone::Tag,
    Tone::Attribute,
    Tone::Variable,
    Tone::Invalid,
];

/// TextMate scope selectors for each tone (the most specific match wins,
/// as in any TextMate theme).
const TONE_SCOPES: &[(&str, Tone)] = &[
    ("comment, punctuation.definition.comment", Tone::Comment),
    (
        "string, punctuation.definition.string, constant.character.escape",
        Tone::String,
    ),
    (
        "constant.numeric, constant.language, constant.character, constant.other",
        Tone::Number,
    ),
    (
        "keyword, storage.modifier, storage.type.function, keyword.operator.new",
        Tone::Keyword,
    ),
    ("keyword.operator", Tone::Plain),
    (
        "entity.name.function, support.function, meta.function-call variable.function",
        Tone::Function,
    ),
    (
        "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, support.type, support.class, storage.type",
        Tone::Type,
    ),
    ("entity.name.tag, markup.heading", Tone::Tag),
    (
        "entity.other.attribute-name, meta.object-literal.key, support.type.property-name",
        Tone::Attribute,
    ),
    (
        "variable.parameter, variable.other.member, variable.language",
        Tone::Variable,
    ),
    ("invalid", Tone::Invalid),
];

pub type Spans = Vec<(Tone, String)>;

#[derive(Debug)]
pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

static GLOBAL: OnceLock<Highlighter> = OnceLock::new();

/// Loads grammars once, including user grammars from `user_dir`. Returns
/// messages about user grammars that could not be loaded. Later calls are
/// no-ops.
pub fn init(user_dir: Option<&Path>) -> Vec<String> {
    let mut warnings = Vec::new();
    if GLOBAL.get().is_none() {
        let hl = Highlighter::new(user_dir, &mut warnings);
        let _ = GLOBAL.set(hl);
    }
    warnings
}

/// The shared highlighter (bundled grammars only if [`init`] was not called).
pub fn get() -> &'static Highlighter {
    GLOBAL.get_or_init(|| Highlighter::new(None, &mut Vec::new()))
}

impl Highlighter {
    fn new(user_dir: Option<&Path>, warnings: &mut Vec<String>) -> Self {
        let mut builder = two_face::syntax::extra_newlines().into_builder();
        if let Some(dir) = user_dir
            && let Ok(entries) = std::fs::read_dir(dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let loaded = if name.ends_with(".tmLanguage.json") {
                    std::fs::read_to_string(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|s| tmlanguage::load(&s))
                } else if name.ends_with(".sublime-syntax") {
                    std::fs::read_to_string(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|s| {
                            syntect::parsing::SyntaxDefinition::load_from_str(&s, true, None)
                                .map_err(|e| e.to_string())
                        })
                } else {
                    continue;
                };
                match loaded {
                    Ok(def) => builder.add(def),
                    Err(e) => warnings.push(format!("{name}: {e}")),
                }
            }
        }
        Self {
            syntaxes: builder.build(),
            theme: tone_theme(),
        }
    }

    /// The grammar for a file, by extension or file name.
    pub fn syntax_for(&self, path: &Path) -> Option<&SyntaxReference> {
        let ext = path.extension().and_then(|e| e.to_str());
        let name = path.file_name().and_then(|n| n.to_str());
        ext.and_then(|e| self.syntaxes.find_syntax_by_extension(e))
            .or_else(|| name.and_then(|n| self.syntaxes.find_syntax_by_extension(n)))
    }

    /// Highlights consecutive lines of one file (state carries across
    /// lines). `None` when no grammar matches the path.
    pub fn highlight(&self, path: &Path, lines: &[&str]) -> Option<Vec<Spans>> {
        let syntax = self.syntax_for(path)?;
        let mut h = HighlightLines::new(syntax, &self.theme);
        let mut out = Vec::with_capacity(lines.len());
        for line in lines {
            let with_nl = format!("{line}\n");
            let spans = match h.highlight_line(&with_nl, &self.syntaxes) {
                Ok(ranges) => ranges
                    .into_iter()
                    .filter_map(|(style, text)| {
                        let text = text.trim_end_matches('\n');
                        (!text.is_empty()).then(|| (tone_of(style.foreground), text.to_owned()))
                    })
                    .collect(),
                // A grammar error must never hide the diff: fall back to plain.
                Err(_) => vec![(Tone::Plain, (*line).to_owned())],
            };
            out.push(spans);
        }
        Some(out)
    }
}

/// A theme whose foreground red channel encodes the [`Tone`] index.
fn tone_theme() -> Theme {
    let encode = |tone: Tone| Color {
        r: TONES
            .iter()
            .position(|t| *t == tone)
            .and_then(|i| u8::try_from(i).ok())
            .unwrap_or(0),
        g: 0,
        b: 0,
        a: 255,
    };
    Theme {
        name: Some("gf-tones".into()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(encode(Tone::Plain)),
            ..ThemeSettings::default()
        },
        scopes: TONE_SCOPES
            .iter()
            .filter_map(|(selector, tone)| {
                Some(ThemeItem {
                    scope: ScopeSelectors::from_str(selector).ok()?,
                    style: StyleModifier {
                        foreground: Some(encode(*tone)),
                        background: None,
                        font_style: None,
                    },
                })
            })
            .collect(),
    }
}

fn tone_of(c: Color) -> Tone {
    TONES.get(usize::from(c.r)).copied().unwrap_or(Tone::Plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tones(spans: &Spans) -> Vec<Tone> {
        spans
            .iter()
            .map(|(t, _)| *t)
            .filter(|t| *t != Tone::Plain)
            .collect()
    }

    #[test]
    fn highlights_rust() {
        let hl = get();
        let out = hl
            .highlight(
                Path::new("src/main.rs"),
                &["fn main() { let s = \"hi\"; // note", "}"],
            )
            .expect("rust grammar");
        let t = tones(&out[0]);
        assert!(t.contains(&Tone::Keyword), "{:?}", out[0]);
        assert!(t.contains(&Tone::String), "{:?}", out[0]);
        assert!(t.contains(&Tone::Comment), "{:?}", out[0]);
        let text: String = out[0].iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(text, "fn main() { let s = \"hi\"; // note");
    }

    #[test]
    fn state_carries_across_lines() {
        let hl = get();
        let out = hl
            .highlight(Path::new("a.rs"), &["/* start", "still comment */ x"])
            .expect("rust");
        assert_eq!(out[1][0].0, Tone::Comment);
    }

    #[test]
    fn unknown_extension_has_no_grammar() {
        assert!(
            get()
                .highlight(Path::new("file.zzzunknown"), &["x"])
                .is_none()
        );
    }
}
