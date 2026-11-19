//! Build aligned rows for a full-file comparison from the loaded Git diff.

use std::sync::Arc;

use gf_core::model::{FileDiff, LineKind};

use super::{Code, scroll_marks};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Row {
    pub before: Option<usize>,
    pub after: Option<usize>,
    pub changed: bool,
}

#[derive(Debug)]
pub(super) struct Comparison {
    pub before: Option<Box<Code>>,
    pub deleted: bool,
    pub rows: Vec<Row>,
    pub marks: Arc<[scroll_marks::Mark]>,
}

impl Comparison {
    pub fn new(file: &Code, before: Option<Code>, diff: &FileDiff) -> Option<Self> {
        let deleted = diff.new_path.is_none();
        let old = if deleted { Some(file) } else { before.as_ref() };
        if diff.is_binary || diff.old_path.is_some() != old.is_some() {
            return None;
        }
        let new = (!deleted).then_some(file);
        let rows = aligned_rows(old, new, diff)?;
        let marks = scroll_marks::collect(rows.iter().map(|row| {
            if row.changed {
                [row.after.is_some(), row.before.is_some()]
            } else {
                [false; 2]
            }
        }));
        Some(Self {
            before: before.map(Box::new),
            deleted,
            rows,
            marks,
        })
    }
}

fn start(line: u32, count: u32) -> Option<usize> {
    Some(if count == 0 {
        line
    } else {
        line.checked_sub(1)?
    } as usize)
}

fn unchanged(
    rows: &mut Vec<Row>,
    old: std::ops::Range<usize>,
    new: std::ops::Range<usize>,
) -> Option<()> {
    if old.len() != new.len() {
        return None;
    }
    rows.extend(old.zip(new).map(|(before, after)| Row {
        before: Some(before),
        after: Some(after),
        changed: false,
    }));
    Some(())
}

fn changed(rows: &mut Vec<Row>, old: std::ops::Range<usize>, new: std::ops::Range<usize>) {
    let count = old.len().max(new.len());
    let mut old = old;
    let mut new = new;
    rows.extend((0..count).map(|_| Row {
        before: old.next(),
        after: new.next(),
        changed: true,
    }));
}

fn aligned_rows(old: Option<&Code>, new: Option<&Code>, diff: &FileDiff) -> Option<Vec<Row>> {
    let old_len = old.map_or(0, |code| code.lines.len());
    let new_len = new.map_or(0, |code| code.lines.len());
    let mut rows = Vec::with_capacity(old_len.max(new_len));
    let (mut old_at, mut new_at) = (0, 0);
    for hunk in &diff.hunks {
        let old_start = start(hunk.old_start, hunk.old_lines)?;
        let new_start = start(hunk.new_start, hunk.new_lines)?;
        if !(old_at..=old_len).contains(&old_start) || !(new_at..=new_len).contains(&new_start) {
            return None;
        }
        unchanged(&mut rows, old_at..old_start, new_at..new_start)?;
        (old_at, new_at) = (old_start, new_start);
        let (mut block_old, mut block_new) = (old_at, new_at);
        for line in &hunk.lines {
            match line.kind {
                LineKind::Removed => {
                    let code = old?;
                    if old_at >= old_len || code.line(old_at) != line.content.trim_end_matches('\r')
                    {
                        return None;
                    }
                    old_at += 1;
                }
                LineKind::Added => {
                    let code = new?;
                    if new_at >= new_len || code.line(new_at) != line.content.trim_end_matches('\r')
                    {
                        return None;
                    }
                    new_at += 1;
                }
                LineKind::Context => {
                    if old_at >= old_len || new_at >= new_len {
                        return None;
                    }
                    changed(&mut rows, block_old..old_at, block_new..new_at);
                    unchanged(&mut rows, old_at..old_at + 1, new_at..new_at + 1)?;
                    old_at += 1;
                    new_at += 1;
                    (block_old, block_new) = (old_at, new_at);
                }
            }
        }
        changed(&mut rows, block_old..old_at, block_new..new_at);
    }
    unchanged(&mut rows, old_at..old_len, new_at..new_len)?;
    Some(rows)
}
