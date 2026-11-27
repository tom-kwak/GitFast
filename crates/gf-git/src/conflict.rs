//! Conflict markers in a work tree file: parsing them into blocks and
//! writing the file back with a chosen side per block.
//!
//! Supports the default `merge` style and `diff3`/`zdiff3` (with the
//! `|||||||` base section). Pure and UI-independent.

/// Which version to keep for a conflict block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Ours,
    Theirs,
    /// Ours followed by theirs.
    Both,
    /// Theirs followed by ours.
    BothTheirsFirst,
    Base,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Text outside conflicts, kept as is.
    Common(String),
    Conflict(Block),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Label after `<<<<<<<` (e.g. `HEAD`).
    pub ours_label: String,
    /// Label after `>>>>>>>` (e.g. `feature`).
    pub theirs_label: String,
    pub ours: String,
    /// Present with the diff3 conflict style.
    pub base: Option<String>,
    pub theirs: String,
    /// The block exactly as it is in the file (markers, labels and line
    /// endings included), written back when no side is chosen.
    raw: String,
}

impl Block {
    fn empty() -> Self {
        Self {
            ours_label: String::new(),
            theirs_label: String::new(),
            ours: String::new(),
            base: None,
            theirs: String::new(),
            raw: String::new(),
        }
    }

    pub fn resolve(&self, choice: Choice) -> String {
        match choice {
            Choice::Ours => self.ours.clone(),
            Choice::Theirs => self.theirs.clone(),
            Choice::Both => format!("{}{}", self.ours, self.theirs),
            Choice::BothTheirsFirst => format!("{}{}", self.theirs, self.ours),
            Choice::Base => self.base.clone().unwrap_or_default(),
        }
    }
}

/// Splits file content into common text and conflict blocks. Text inside
/// blocks keeps its line endings. Returns `None` when the markers are
/// malformed (e.g. a `<<<<<<<` without `>>>>>>>`), so callers can fall back
/// to plain editing.
pub fn parse(text: &str) -> Option<Vec<Segment>> {
    parse_with_width(text, 7)
}

pub fn parse_with_width(text: &str, width: usize) -> Option<Vec<Segment>> {
    #[derive(PartialEq)]
    enum In {
        Common,
        Ours,
        Base,
        Theirs,
    }
    let mut segments = Vec::new();
    let mut common = String::new();
    let mut state = In::Common;
    let mut block = Block::empty();
    let marker = |line: &str, m: u8| {
        let prefix = line.as_bytes().iter().take_while(|b| **b == m).count();
        (prefix == width)
            .then(|| &line[prefix..])
            .filter(|rest| {
                rest.is_empty()
                    || rest.starts_with(' ')
                    || rest.starts_with('\n')
                    || rest.starts_with('\r')
            })
            .map(|rest| rest.trim().to_owned())
    };

    for line in text.split_inclusive('\n') {
        if state != In::Common {
            block.raw.push_str(line);
        }
        match state {
            In::Common => {
                if let Some(label) = marker(line, b'<') {
                    if !common.is_empty() {
                        segments.push(Segment::Common(std::mem::take(&mut common)));
                    }
                    block.ours_label = label;
                    block.raw.push_str(line);
                    state = In::Ours;
                } else {
                    common.push_str(line);
                }
            }
            In::Ours => {
                if marker(line, b'|').is_some() {
                    block.base = Some(String::new());
                    state = In::Base;
                } else if marker(line, b'=').is_some() {
                    state = In::Theirs;
                } else {
                    block.ours.push_str(line);
                }
            }
            In::Base => {
                if marker(line, b'=').is_some() {
                    state = In::Theirs;
                } else if let Some(b) = block.base.as_mut() {
                    b.push_str(line);
                }
            }
            In::Theirs => {
                if let Some(label) = marker(line, b'>') {
                    block.theirs_label = label;
                    segments.push(Segment::Conflict(std::mem::replace(
                        &mut block,
                        Block::empty(),
                    )));
                    state = In::Common;
                } else {
                    block.theirs.push_str(line);
                }
            }
        }
    }
    if state != In::Common {
        return None;
    }
    if !common.is_empty() {
        segments.push(Segment::Common(common));
    }
    Some(segments)
}

/// Also detects incomplete marker sections in an edited candidate.
pub fn has_markers(text: &str, width: usize) -> bool {
    text.lines().any(|line| {
        b"<>|".iter().copied().any(|marker| {
            let len = line.as_bytes().iter().take_while(|b| **b == marker).count();
            len == width && (line.len() == len || line.as_bytes()[len] == b' ')
        })
    })
}

pub fn conflict_count(segments: &[Segment]) -> usize {
    segments
        .iter()
        .filter(|s| matches!(s, Segment::Conflict(_)))
        .count()
}

/// Rebuilds the file. `choices[i]` resolves the i-th conflict; blocks
/// without a choice keep their markers so nothing is silently lost.
pub fn render(segments: &[Segment], choices: &[Option<Choice>]) -> String {
    let mut out = String::new();
    let mut n = 0;
    for s in segments {
        match s {
            Segment::Common(t) => out.push_str(t),
            Segment::Conflict(b) => {
                match choices.get(n).copied().flatten() {
                    Some(c) => out.push_str(&b.resolve(c)),
                    None => out.push_str(&b.raw),
                }
                n += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "\
fn a() {}
<<<<<<< HEAD
let x = 1;
=======
let x = 2;
>>>>>>> feature
fn b() {}
<<<<<<< HEAD
ours
||||||| base
base
=======
theirs
>>>>>>> feature
";

    #[test]
    fn parses_blocks_with_and_without_base() {
        let s = parse(TEXT).expect("parse");
        assert_eq!(conflict_count(&s), 2);
        let Segment::Conflict(b) = &s[1] else {
            panic!("conflict")
        };
        assert_eq!(
            (b.ours_label.as_str(), b.theirs_label.as_str()),
            ("HEAD", "feature")
        );
        assert_eq!(
            (b.ours.as_str(), b.theirs.as_str()),
            ("let x = 1;\n", "let x = 2;\n")
        );
        assert_eq!(b.base, None);
        let Segment::Conflict(b2) = &s[3] else {
            panic!("conflict")
        };
        assert_eq!(b2.base.as_deref(), Some("base\n"));
    }

    #[test]
    fn renders_choices_and_keeps_unresolved_markers() {
        let s = parse(TEXT).expect("parse");
        let out = render(&s, &[Some(Choice::Theirs), Some(Choice::Both)]);
        assert_eq!(out, "fn a() {}\nlet x = 2;\nfn b() {}\nours\ntheirs\n");
        let partial = render(&s, &[Some(Choice::Ours), None]);
        assert!(
            partial.contains("let x = 1;")
                && partial.contains(
                    "<<<<<<< HEAD\nours\n||||||| base\nbase\n=======\ntheirs\n>>>>>>> feature\n"
                )
        );
        assert_eq!(parse(&partial).map(|s| conflict_count(&s)), Some(1));
    }

    #[test]
    fn unresolved_blocks_are_written_back_unchanged() {
        // Labels, the base label and CRLF line endings all survive.
        let text = "a\r\n<<<<<<< HEAD\r\nx\r\n||||||| merged common\r\nb\r\n=======\r\ny\r\n>>>>>>> topic\r\nz\r\n";
        let s = parse(text).expect("parse");
        assert_eq!(render(&s, &[None]), text);
    }

    #[test]
    fn malformed_markers_are_rejected() {
        assert!(parse("<<<<<<< HEAD\nx\n").is_none());
        assert_eq!(parse("no conflicts\n").map(|s| conflict_count(&s)), Some(0));
        // A line merely starting with the marker characters is not a marker.
        assert_eq!(
            parse("<<<<<<<<< not a marker\n").map(|s| conflict_count(&s)),
            Some(0)
        );
    }
}
