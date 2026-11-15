//! Commit graph layout: assigns each commit a lane and produces one row of
//! box-drawing glyphs per commit, terminal style:
//!
//! ```text
//! ◉─╮   merge
//! │ ●   feature work
//! ● │   main work
//! ├─╯
//! ●     base
//! ```
//!
//! Besides the glyphs, every row carries [`Edge`]s — line segments from
//! the row's top edge, through the commit dot, to its bottom edge — so a UI
//! can draw continuous vector lines (no gaps between rows) at any lane
//! spacing.
//!
//! Input must be in an order where children come before parents
//! (`git log --topo-order` / `--date-order`). Pure and UI-independent.

use gf_core::model::{Commit, Oid};

/// One drawn character with the index of the lane color it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub color: usize,
}

const EMPTY: Cell = Cell { ch: ' ', color: 0 };

/// Vertical position within a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Top,
    Center,
    Bottom,
}

/// A line segment inside one row, from `(from_lane, from)` to
/// `(to_lane, to)`, in lane color `color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from_lane: usize,
    pub from: Anchor,
    pub to_lane: usize,
    pub to: Anchor,
    pub color: usize,
}

/// Glyphs for one commit row. Lane `i` is drawn at `cells[2 * i]`; the
/// cell after it holds the horizontal connector towards lane `i + 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// Lane of the commit itself.
    pub lane: usize,
    /// Lane color of the commit dot.
    pub color: usize,
    pub is_merge: bool,
    /// Number of lanes touched by this row.
    pub width: usize,
    pub edges: Vec<Edge>,
    pub cells: Vec<Cell>,
}

impl GraphRow {
    /// The row as plain text (for tests and debugging).
    pub fn text(&self) -> String {
        self.cells
            .iter()
            .map(|c| c.ch)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }
}

#[derive(Debug, Clone)]
struct Lane {
    expects: Oid,
    color: usize,
}

/// Lays out `commits` (children before parents). `colors` is how many
/// distinct lane colors the UI has; lanes cycle through them.
pub fn layout(commits: &[Commit], colors: usize) -> Vec<GraphRow> {
    let colors = colors.max(1);
    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut next_color = 0usize;
    let mut new_color = || {
        let c = next_color % colors;
        next_color += 1;
        c
    };
    let mut rows = Vec::with_capacity(commits.len());

    for commit in commits {
        let before = lanes.clone();

        // Where this commit is drawn: the lane already waiting for it, or a
        // free slot (a branch tip nobody has pointed at yet).
        let col = match lanes
            .iter()
            .position(|l| l.as_ref().is_some_and(|l| l.expects == commit.id))
        {
            Some(c) => c,
            None => {
                let slot = lanes
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(lanes.len());
                let lane = Lane {
                    expects: commit.id.clone(),
                    color: new_color(),
                };
                if slot == lanes.len() {
                    lanes.push(Some(lane));
                } else {
                    lanes[slot] = Some(lane);
                }
                slot
            }
        };
        let color = lanes[col].as_ref().map_or(0, |l| l.color);

        // Other lanes that were also waiting for this commit converge here.
        let converging: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(i, l)| *i != col && l.as_ref().is_some_and(|l| l.expects == commit.id))
            .map(|(i, _)| i)
            .collect();
        for &i in &converging {
            lanes[i] = None;
        }

        // Parents: the first continues this lane, others join or open lanes.
        // (target lane, opened new lane?)
        let mut links: Vec<(usize, bool)> = Vec::new();
        lanes[col] = None;
        for (n, parent) in commit.parents.iter().enumerate() {
            if n == 0 {
                // The first parent always continues this lane, even if another
                // lane waits for it too; they converge at the parent's row.
                lanes[col] = Some(Lane {
                    expects: parent.clone(),
                    color,
                });
            } else if let Some(j) = lanes
                .iter()
                .position(|l| l.as_ref().is_some_and(|l| &l.expects == parent))
            {
                links.push((j, false));
            } else {
                let slot = lanes
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(lanes.len());
                let lane = Lane {
                    expects: parent.clone(),
                    color: new_color(),
                };
                if slot == lanes.len() {
                    lanes.push(Some(lane));
                } else {
                    lanes[slot] = Some(lane);
                }
                links.push((slot, true));
            }
        }

        // Draw.
        let width = before.len().max(lanes.len()).max(col + 1);
        let mut cells = vec![EMPTY; width * 2];
        let lane_color = |i: usize| {
            lanes
                .get(i)
                .and_then(|l| l.as_ref())
                .or_else(|| before.get(i).and_then(|l| l.as_ref()))
                .map_or(color, |l| l.color)
        };
        for (i, l) in before.iter().enumerate() {
            if let Some(l) = l {
                cells[2 * i] = Cell {
                    ch: '│',
                    color: l.color,
                };
            }
        }
        let connect = |cells: &mut Vec<Cell>, target: usize, end_glyph: char, link_color: usize| {
            let (lo, hi) = if target < col {
                (target, col)
            } else {
                (col, target)
            };
            for k in lo..hi {
                cells[2 * k + 1] = Cell {
                    ch: '─',
                    color: link_color,
                };
                if k > lo && cells[2 * k].ch == '│' {
                    cells[2 * k] = Cell {
                        ch: '┼',
                        color: cells[2 * k].color,
                    };
                } else if k > lo {
                    cells[2 * k] = Cell {
                        ch: '─',
                        color: link_color,
                    };
                }
            }
            cells[2 * target] = Cell {
                ch: end_glyph,
                color: link_color,
            };
        };
        for &i in &converging {
            let glyph = if i > col { '╯' } else { '╰' };
            let c = before[i].as_ref().map_or(color, |l| l.color);
            connect(&mut cells, i, glyph, c);
        }
        for &(j, opened) in &links {
            let glyph = match (opened, j > col) {
                (true, true) => '╮',
                (true, false) => '╭',
                (false, true) => '┤',
                (false, false) => '├',
            };
            connect(&mut cells, j, glyph, lane_color(j));
        }
        cells[2 * col] = Cell {
            ch: if commit.is_merge() { '◉' } else { '●' },
            color,
        };

        // Vector segments: lanes coming in from above either pass through
        // or end at this commit; parents leave towards the bottom.
        let mut edges = Vec::new();
        for (i, l) in before.iter().enumerate() {
            let Some(l) = l else { continue };
            let (to_lane, to) = if l.expects == commit.id {
                (col, Anchor::Center)
            } else {
                (i, Anchor::Bottom)
            };
            edges.push(Edge {
                from_lane: i,
                from: Anchor::Top,
                to_lane,
                to,
                color: l.color,
            });
        }
        if !commit.parents.is_empty() && lanes.get(col).is_some_and(Option::is_some) {
            edges.push(Edge {
                from_lane: col,
                from: Anchor::Center,
                to_lane: col,
                to: Anchor::Bottom,
                color,
            });
        }
        for &(j, _) in &links {
            edges.push(Edge {
                from_lane: col,
                from: Anchor::Center,
                to_lane: j,
                to: Anchor::Bottom,
                color: lane_color(j),
            });
        }

        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        rows.push(GraphRow {
            lane: col,
            color,
            is_merge: commit.is_merge(),
            width,
            edges,
            cells,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use gf_core::model::{Signature, Timestamp};

    fn oid(n: u8) -> Oid {
        format!("{n:040x}").parse().expect("oid")
    }

    fn commit(id: u8, parents: &[u8]) -> Commit {
        let sig = Signature {
            name: String::new(),
            email: String::new(),
            time: Timestamp {
                seconds: 0,
                offset_minutes: 0,
            },
        };
        Commit {
            id: oid(id),
            parents: parents.iter().map(|p| oid(*p)).collect(),
            author: sig.clone(),
            committer: sig,
            summary: String::new(),
            body: String::new(),
        }
    }

    fn texts(commits: &[Commit]) -> Vec<String> {
        layout(commits, 6).iter().map(GraphRow::text).collect()
    }

    #[test]
    fn linear_history_is_one_lane() {
        let rows = texts(&[commit(3, &[2]), commit(2, &[1]), commit(1, &[])]);
        assert_eq!(rows, ["●", "●", "●"]);
    }

    #[test]
    fn merge_opens_and_closes_a_lane() {
        // M merges F into main; C is main's work; B is the fork point.
        let rows = texts(&[
            commit(5, &[4, 3]), // M
            commit(3, &[2]),    // F (feature)
            commit(4, &[2]),    // C (main)
            commit(2, &[1]),    // B
            commit(1, &[]),     // A
        ]);
        assert_eq!(rows, ["◉─╮", "│ ●", "● │", "●─╯", "●"]);
    }

    #[test]
    fn independent_branch_tips_get_their_own_lanes() {
        // Two tips (5 and 4) sharing parent 3.
        let rows = texts(&[commit(5, &[3]), commit(4, &[3]), commit(3, &[])]);
        assert_eq!(rows, ["●", "│ ●", "●─╯"]);
    }

    #[test]
    fn lanes_cycle_colors() {
        let rows = layout(
            &[
                commit(5, &[4, 3]),
                commit(3, &[1]),
                commit(4, &[1]),
                commit(1, &[]),
            ],
            2,
        );
        let merge_row = &rows[0];
        assert_ne!(merge_row.cells[0].color, merge_row.cells[2].color);
    }

    #[test]
    fn edges_connect_rows() {
        // Same history as merge_opens_and_closes_a_lane.
        let rows = layout(
            &[
                commit(5, &[4, 3]),
                commit(3, &[2]),
                commit(4, &[2]),
                commit(2, &[1]),
                commit(1, &[]),
            ],
            6,
        );
        let e = |r: &GraphRow| {
            r.edges
                .iter()
                .map(|e| (e.from_lane, e.from, e.to_lane, e.to))
                .collect::<Vec<_>>()
        };
        use Anchor::{Bottom, Center, Top};
        // Merge tip: nothing above; continues down lane 0 and opens lane 1.
        assert_eq!(
            e(&rows[0]),
            [(0, Center, 0, Bottom), (0, Center, 1, Bottom)]
        );
        assert!(rows[0].is_merge);
        // Feature commit on lane 1: lane 0 passes through.
        assert_eq!(
            e(&rows[1]),
            [
                (0, Top, 0, Bottom),
                (1, Top, 1, Center),
                (1, Center, 1, Bottom)
            ]
        );
        // Fork point: lane 1 converges into the commit on lane 0.
        assert_eq!(
            e(&rows[3]),
            [
                (0, Top, 0, Center),
                (1, Top, 0, Center),
                (0, Center, 0, Bottom)
            ]
        );
        // Root: ends here.
        assert_eq!(e(&rows[4]), [(0, Top, 0, Center)]);
        // Every bottom of a row meets a top of the next row in the same lane.
        for pair in rows.windows(2) {
            let bottoms: std::collections::BTreeSet<_> = pair[0]
                .edges
                .iter()
                .filter(|e| e.to == Bottom)
                .map(|e| e.to_lane)
                .collect();
            let tops: std::collections::BTreeSet<_> = pair[1]
                .edges
                .iter()
                .filter(|e| e.from == Top)
                .map(|e| e.from_lane)
                .collect();
            assert_eq!(bottoms, tops);
        }
    }
}
