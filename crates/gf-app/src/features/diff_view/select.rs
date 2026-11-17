//! Text selection in a diff, as in any text viewer: from where the mouse
//! went down to where it was let go, character by character. The code and
//! its `+` / `-` / ` ` marker can be selected; the line numbers are the
//! gutter and never are.
//!
//! A position in a line counts characters of its *selectable text* (marker
//! then code): 0 is before the marker, 1 is where the code starts, `1 + n`
//! after `n` characters of code. Where a position is on screen is the
//! line widget's business (`widgets::selectable_line`).

use gf_core::model::LineKind;

/// `(file, hunk, line)` index of a diff line.
pub type LineKey = (usize, usize, usize);

/// A point in the selectable text of a line (see the module docs).
pub type Pos = (LineKey, usize);

/// One selected stretch: from where the mouse went down (`anchor`) to where
/// it is (`head`); either may come first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub anchor: Pos,
    pub head: Pos,
}

impl Range {
    pub fn caret(at: Pos) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }

    /// Start and end, in the order they are shown.
    pub fn ordered(&self) -> (Pos, Pos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The part of line `key` (with `len` positions) this range covers:
    /// `start..end`, and whether the line break after it is selected too
    /// (the range goes on to a later line). `None` when nothing of it is.
    pub fn span(&self, key: LineKey, len: usize) -> Option<(usize, usize, bool)> {
        let ((sk, sp), (ek, ep)) = self.ordered();
        if key < sk || key > ek {
            return None;
        }
        let start = if key == sk { sp.min(len) } else { 0 };
        let (end, newline) = if key == ek {
            (ep.min(len), false)
        } else {
            (len, true)
        };
        (end > start || newline).then_some((start, end, newline))
    }
}

/// Positions in a line: its marker plus each character of code.
pub fn len(content: &str) -> usize {
    1 + content.chars().count()
}

/// The selectable text of a line: marker then code.
pub fn text(kind: LineKind, content: &str) -> String {
    let mut s = String::with_capacity(content.len() + 1);
    s.push(marker(kind));
    s.push_str(content);
    s
}

/// A line's marker: `+` added, `-` removed, ` ` unchanged.
pub fn marker(kind: LineKind) -> char {
    match kind {
        LineKind::Context => ' ',
        LineKind::Added => '+',
        LineKind::Removed => '-',
    }
}

/// The text of `range` over `lines` (the keys from its start line to its
/// end line, in order, each with its kind and code): exactly the selected
/// characters, line breaks where it crosses lines.
pub fn range_text<'a>(
    range: &Range,
    lines: impl Iterator<Item = (LineKey, LineKind, &'a str)>,
) -> String {
    let mut out = String::new();
    for (key, kind, content) in lines {
        let Some((s, e, newline)) = range.span(key, len(content)) else {
            continue;
        };
        out.extend(text(kind, content).chars().skip(s).take(e - s));
        if newline {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const K0: LineKey = (0, 0, 0);
    const K1: LineKey = (0, 0, 1);
    const K2: LineKey = (0, 0, 2);

    #[test]
    fn a_range_covers_exactly_what_was_dragged_over() {
        // From the 2nd character of line 0 to the 1st of line 2.
        let r = Range {
            anchor: (K2, 2),
            head: (K0, 2),
        };
        assert_eq!(r.span(K0, 4), Some((2, 4, true)));
        assert_eq!(r.span(K1, 3), Some((0, 3, true)));
        assert_eq!(r.span(K2, 4), Some((0, 2, false)));
        assert_eq!(r.span((0, 0, 3), 4), None);
        let lines = [
            (K0, LineKind::Context, "abc"),
            (K1, LineKind::Removed, "de"),
            (K2, LineKind::Added, "fgh"),
        ];
        assert_eq!(range_text(&r, lines.into_iter()), "bc\n-de\n+f");

        // Ending at the very start of a line selects only the break before.
        let r = Range {
            anchor: (K0, 0),
            head: (K1, 0),
        };
        assert_eq!(range_text(&r, lines.into_iter()), " abc\n");
        assert_eq!(r.span(K1, 3), None);
        assert!(Range::caret((K0, 1)).span(K0, 4).is_none());
    }
}
