//! The worktree draft and the aligned rows of each conflicted region.

use std::ops::Range;
use std::sync::Arc;

use gf_git::ConflictDocument;
use gf_git::conflict::{self, Choice, Segment};
use gix_imara_diff::{Algorithm, Diff, InternedInput};

const CONTEXT: usize = 3;
const MAX_LINES: usize = 30_000;

#[derive(Debug)]
pub(super) struct Block {
    pub segment: usize,
    pub line: usize,
    pub pairs: Vec<Pair>,
    pub sides: [Vec<String>; 2],
    pub base: Vec<String>,
}

#[derive(Debug)]
pub(super) struct Pair {
    pub ours: Option<usize>,
    pub theirs: Option<usize>,
    pub changed: bool,
}

#[derive(Debug)]
pub(super) struct AlignedResult {
    pub source: Option<usize>,
    pub result: Option<usize>,
    pub changed: bool,
}

impl Block {
    pub fn align_result(
        &self,
        result: &[String],
        resolution: Option<&Resolution>,
    ) -> Vec<AlignedResult> {
        match resolution {
            None | Some(Resolution::Side(Choice::Ours | Choice::Theirs)) => self
                .pairs
                .iter()
                .enumerate()
                .map(|(i, pair)| AlignedResult {
                    source: Some(i),
                    result: match resolution {
                        Some(Resolution::Side(Choice::Ours)) => pair.ours,
                        Some(Resolution::Side(Choice::Theirs)) => pair.theirs,
                        _ => None,
                    },
                    changed: pair.changed,
                })
                .collect(),
            Some(_) => {
                let anchor = self.pairs.iter().map(|pair| {
                    pair.ours
                        .map(|line| self.sides[0][line].as_str())
                        .or_else(|| pair.theirs.map(|line| self.sides[1][line].as_str()))
                        .unwrap_or_default()
                });
                align_lines(anchor, result.iter().map(String::as_str))
                    .into_iter()
                    .map(|pair| AlignedResult {
                        source: pair.ours,
                        result: pair.theirs,
                        changed: pair.changed || pair.ours.is_some_and(|i| self.pairs[i].changed),
                    })
                    .collect()
            }
        }
    }
}

#[derive(Debug)]
pub struct Prepared {
    pub(super) original: Arc<ConflictDocument>,
    pub(super) segments: Option<Vec<Segment>>,
    pub(super) blocks: Vec<Block>,
    pub(super) common: Vec<Vec<String>>,
    pub(super) common_sources: [Vec<Vec<Option<String>>>; 2],
    pub(super) base: Vec<String>,
}

impl Prepared {
    pub fn new(document: ConflictDocument) -> Self {
        let segments = document
            .worktree
            .as_deref()
            .filter(|text| text.lines().take(MAX_LINES + 1).count() <= MAX_LINES)
            .and_then(|text| conflict::parse_with_width(text, document.marker_width));
        let mut blocks = Vec::new();
        let mut common = Vec::new();
        let mut line = 1;
        for (segment, entry) in segments.iter().flatten().enumerate() {
            match entry {
                Segment::Common(text) => {
                    let lines: Vec<_> = text.lines().map(str::to_owned).collect();
                    line += lines.len();
                    common.push(lines);
                }
                Segment::Conflict(block) => {
                    blocks.push(Block {
                        segment,
                        line,
                        pairs: align(&block.ours, &block.theirs),
                        sides: [&block.ours, &block.theirs]
                            .map(|text| text.lines().map(str::to_owned).collect()),
                        base: block
                            .base
                            .as_deref()
                            .unwrap_or_default()
                            .lines()
                            .map(str::to_owned)
                            .collect(),
                    });
                    line += 3
                        + block.ours.lines().count()
                        + block.theirs.lines().count()
                        + block
                            .base
                            .as_ref()
                            .map_or(0, |base| 1 + base.lines().count());
                    common.push(Vec::new());
                }
            }
        }
        let base = document.sources[0]
            .as_deref()
            .unwrap_or_default()
            .lines()
            .take(MAX_LINES)
            .map(str::to_owned)
            .collect();
        let common_sources = [Choice::Ours, Choice::Theirs].map(|side| {
            let index = if side == Choice::Ours { 1 } else { 2 };
            let source = document.sources[index].as_deref().unwrap_or_default();
            project_context(
                segments.as_deref().unwrap_or_default(),
                &blocks,
                side,
                source,
            )
        });
        Self {
            original: Arc::new(document),
            segments,
            blocks,
            common,
            common_sources,
            base,
        }
    }

    pub(super) fn block(&self, n: usize) -> Option<&conflict::Block> {
        match self.segments.as_ref()?.get(self.blocks.get(n)?.segment)? {
            Segment::Conflict(block) => Some(block),
            Segment::Common(_) => None,
        }
    }

    pub(super) fn has_following_content(&self, n: usize) -> bool {
        self.segments
            .as_ref()
            .zip(self.blocks.get(n))
            .is_some_and(|(segments, block)| block.segment + 1 < segments.len())
    }

    pub(super) fn resolved_text(&self, n: usize, resolution: &Resolution) -> Option<String> {
        let text = match resolution {
            Resolution::Edited(text) => return Some(text.clone()),
            Resolution::Side(choice) => {
                let mut text = self.block(n)?.resolve(*choice);
                let stage = match choice {
                    Choice::Ours | Choice::BothTheirsFirst => 1,
                    Choice::Theirs | Choice::Both => 2,
                    Choice::Base => 0,
                };
                let source = self.original.sources[stage].as_deref();
                if !self.has_following_content(n)
                    && let Some(source) = source.filter(|src| !src.ends_with('\n'))
                    && let Some(tail) = text.lines().next_back()
                    && !tail.is_empty()
                    && source.ends_with(tail)
                {
                    let end = text
                        .strip_suffix("\r\n")
                        .or_else(|| text.strip_suffix('\n'))
                        .map(str::len);
                    if let Some(end) = end {
                        text.truncate(end);
                    }
                }
                text
            }
        };
        Some(text)
    }
}

fn project_context(
    segments: &[Segment],
    blocks: &[Block],
    side: Choice,
    source: &str,
) -> Vec<Vec<Option<String>>> {
    let count = source.lines().take(MAX_LINES + 1).count();
    if count == 0 || count > MAX_LINES || blocks.is_empty() {
        return vec![Vec::new(); segments.len()];
    }
    let projected = conflict::render(segments, &vec![Some(side); blocks.len()]);
    let source_lines: Vec<_> = source.lines().collect();
    let mut original = vec![None; projected.lines().count()];
    for pair in align(source, &projected) {
        if let (Some(from), Some(to)) = (pair.ours, pair.theirs) {
            original[to] = Some(source_lines[from]);
        }
    }
    let mut line = 0;
    segments
        .iter()
        .map(|segment| match segment {
            Segment::Common(common) => {
                let end = line + common.lines().count();
                let lines = original
                    .get(line..end)
                    .unwrap_or_default()
                    .iter()
                    .map(|text| text.map(str::to_owned))
                    .collect();
                line = end;
                lines
            }
            Segment::Conflict(block) => {
                line += if side == Choice::Ours {
                    block.ours.lines().count()
                } else {
                    block.theirs.lines().count()
                };
                Vec::new()
            }
        })
        .collect()
}

/// Keep unchanged lines paired. Replacement groups follow the order on each
/// side; a missing cell keeps an insertion/deletion from shifting later rows.
fn align(ours: &str, theirs: &str) -> Vec<Pair> {
    align_lines(ours.lines(), theirs.lines())
}

fn align_lines<'a>(
    ours: impl Iterator<Item = &'a str>,
    theirs: impl Iterator<Item = &'a str>,
) -> Vec<Pair> {
    let mut input = InternedInput::default();
    input.update_before(ours);
    input.update_after(theirs);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let mut pairs = Vec::new();
    let (mut before, mut after) = (0, 0);
    for hunk in diff.hunks() {
        let left = hunk.before.start as usize..hunk.before.end as usize;
        let right = hunk.after.start as usize..hunk.after.end as usize;
        append(&mut pairs, before..left.start, after..right.start, false);
        before = left.end;
        after = right.end;
        append(&mut pairs, left, right, true);
    }
    append(
        &mut pairs,
        before..input.before.len(),
        after..input.after.len(),
        false,
    );
    pairs
}

fn append(out: &mut Vec<Pair>, ours: Range<usize>, theirs: Range<usize>, changed: bool) {
    for i in 0..ours.len().max(theirs.len()) {
        out.push(Pair {
            ours: ours.clone().nth(i),
            theirs: theirs.clone().nth(i),
            changed,
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Resolution {
    Side(Choice),
    Edited(String),
}

#[derive(Debug)]
pub(super) enum Row {
    Common { segment: usize, line: usize },
    Heading(usize),
    Code { block: usize, index: usize },
    Fold(usize),
}

#[derive(Debug, Default)]
pub(super) struct Layout {
    pub rows: Vec<Row>,
    pub headings: Vec<usize>,
    pub differences: Vec<Vec<usize>>,
}

impl Layout {
    pub fn new(file: &Prepared, expanded: bool, aligned: &[Vec<AlignedResult>]) -> Self {
        let mut layout = Self::default();
        let mut blocks = file.blocks.iter().enumerate().peekable();
        for (segment, lines) in file.common.iter().enumerate() {
            if blocks
                .peek()
                .is_some_and(|(_, block)| block.segment == segment)
            {
                if let Some((n, _)) = blocks.next() {
                    layout.block(n, &aligned[n], expanded);
                }
            } else {
                layout.run(lines.len(), expanded, |line| Row::Common { segment, line });
            }
        }
        layout
    }

    fn block(&mut self, n: usize, pairs: &[AlignedResult], expanded: bool) {
        self.headings.push(self.rows.len());
        self.rows.push(Row::Heading(n));
        let mut differences = Vec::new();
        let mut start = 0;
        while start < pairs.len() {
            let changed = pairs[start].changed;
            let end = start
                + pairs[start..]
                    .iter()
                    .take_while(|pair| pair.changed == changed)
                    .count();
            if changed {
                differences.push(self.rows.len());
            }
            self.run(end - start, expanded || changed, |offset| Row::Code {
                block: n,
                index: start + offset,
            });
            start = end;
        }
        self.differences.push(differences);
    }

    fn run(&mut self, len: usize, expanded: bool, row: impl Fn(usize) -> Row) {
        if expanded || len <= 2 * CONTEXT + 1 {
            self.rows.extend((0..len).map(row));
        } else {
            self.rows.extend((0..CONTEXT).map(&row));
            self.rows.push(Row::Fold(len - 2 * CONTEXT));
            self.rows.extend((len - CONTEXT..len).map(row));
        }
    }
}

pub(super) fn candidate(file: &Prepared, resolutions: &[Option<Resolution>]) -> Option<String> {
    let mut text = String::new();
    let mut next = resolutions.iter().enumerate();
    for segment in file.segments.as_ref()? {
        match segment {
            Segment::Common(common) => text.push_str(common),
            Segment::Conflict(_) => {
                let (n, choice) = next.next()?;
                text.push_str(&file.resolved_text(n, choice.as_ref()?)?);
            }
        }
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_changes_in_one_conflict_stay_separately_navigable() {
        let ours = (0..100).map(|i| format!("line {i}\n")).collect::<String>();
        let theirs = ours
            .replace("line 10\n", "inserted\nline 10\n")
            .replace("line 40\n", "forty\n")
            .replace("line 80\n", "");
        let mut layout = Layout::default();
        let pairs = align(&ours, &theirs);
        let block = Block {
            segment: 0,
            line: 1,
            pairs,
            sides: [&ours, &theirs].map(|text| text.lines().map(str::to_owned).collect()),
            base: Vec::new(),
        };
        layout.block(0, &block.align_result(&[], None), false);
        assert_eq!(layout.differences[0].len(), 3);
        assert!(layout.rows.len() < 40);
        assert!(
            block
                .pairs
                .iter()
                .any(|row| row.ours.is_none() && row.theirs.is_some())
        );
        assert!(
            block
                .pairs
                .iter()
                .any(|row| row.ours.is_some() && row.theirs.is_none())
        );
    }
}
