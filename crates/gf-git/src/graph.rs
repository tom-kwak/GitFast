//! Commit graph layout: assigns each commit a lane and produces one row
//! per commit. [`layout_text`] draws the rows terminal style:
//!
//! ```text
//! ◉─╮   merge
//! │ ●   feature work
//! ● │   main work
//! ├─╯
//! ●     base
//! ```
//!
//! [`layout`] gives every row [`Edge`]s instead — line segments from the
//! row's top edge, through the commit dot, to its bottom edge — so a UI can
//! draw continuous vector lines (no gaps between rows) at any lane spacing.
//!
//! Input must be in an order where children come before parents
//! (`git log --topo-order` / `--date-order`). Pure and UI-independent.

use gf_core::model::{Commit, Oid};

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

/// One commit row: the commit's lane and the line segments through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// Lane of the commit itself.
    pub lane: usize,
    /// Lane color of the commit dot.
    pub color: usize,
    pub is_merge: bool,
    /// Number of lanes touched by this row.
    pub width: usize,
    /// Segments with at least one end in the first `max_lanes` lanes (see
    /// [`layout`]).
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone)]
struct Lane {
    expects: Oid,
    color: usize,
}

/// What a row needs to know about a lane coming in from above.
#[derive(Debug, Clone, Copy)]
struct Above {
    /// The lane was waiting for this row's commit: it ends here.
    ends_here: bool,
    color: usize,
}

/// Lays out `commits` (children before parents). `colors` is how many
/// distinct lane colors the UI has; lanes cycle through them.
///
/// Segments with both ends at lane `max_lanes` or past it are left out: a
/// UI that draws only the first `max_lanes` lanes never shows them, and a
/// wide history (hundreds of lanes) would otherwise keep hundreds of
/// segments per row.
pub fn layout(commits: &[Commit], colors: usize, max_lanes: usize) -> Vec<GraphRow> {
    Layout::new(colors, max_lanes).rows(commits)
}

/// Each row as box-drawing glyphs, terminal style (for tests and
/// debugging).
pub fn layout_text(commits: &[Commit], colors: usize) -> Vec<String> {
    let mut texts = Vec::with_capacity(commits.len());
    Layout::new(colors, usize::MAX).walk(commits, Some(&mut texts));
    texts
}

/// The lanes open after the rows laid out so far. [`Layout::rows`] lays out
/// the commits that come next in the same order — the next page of a
/// history — as if they had been laid out together with the first ones.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    lanes: Vec<Option<Lane>>,
    next_color: usize,
    colors: usize,
    max_lanes: usize,
}

impl Layout {
    /// No rows yet; `colors` and `max_lanes` as for [`layout`].
    pub fn new(colors: usize, max_lanes: usize) -> Self {
        Self {
            lanes: Vec::new(),
            next_color: 0,
            colors: colors.max(1),
            max_lanes,
        }
    }

    /// Rows of `commits`, continuing from the rows laid out before.
    pub fn rows(&mut self, commits: &[Commit]) -> Vec<GraphRow> {
        self.walk(commits, None)
    }

    fn walk(&mut self, commits: &[Commit], texts: Option<&mut Vec<String>>) -> Vec<GraphRow> {
        let Self {
            lanes,
            next_color,
            colors,
            max_lanes,
        } = self;
        walk(lanes, next_color, *colors, *max_lanes, commits, texts)
    }
}

fn walk(
    lanes: &mut Vec<Option<Lane>>,
    next_color: &mut usize,
    colors: usize,
    max_lanes: usize,
    commits: &[Commit],
    mut texts: Option<&mut Vec<String>>,
) -> Vec<GraphRow> {
    let colors = colors.max(1);
    let mut new_color = || {
        let c = *next_color % colors;
        *next_color += 1;
        c
    };
    let mut rows = Vec::with_capacity(commits.len());
    let mut before: Vec<Option<Above>> = Vec::new();

    for commit in commits {
        before.clear();
        before.extend(lanes.iter().map(|l| {
            l.as_ref().map(|l| Above {
                ends_here: l.expects == commit.id,
                color: l.color,
            })
        }));

        // Where this commit is drawn: the lane already waiting for it, or a
        // free slot (a branch tip nobody has pointed at yet).
        let col = match before.iter().position(|l| l.is_some_and(|l| l.ends_here)) {
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
        let converging: Vec<usize> = before
            .iter()
            .enumerate()
            .filter(|(i, l)| *i != col && l.is_some_and(|l| l.ends_here))
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

        let width = before.len().max(lanes.len()).max(col + 1);
        let lane_color = |i: usize| {
            lanes
                .get(i)
                .and_then(|l| l.as_ref())
                .map(|l| l.color)
                .or_else(|| before.get(i).copied().flatten().map(|l| l.color))
                .unwrap_or(color)
        };
        if let Some(texts) = texts.as_deref_mut() {
            texts.push(glyphs(commit, col, width, &before, &converging, &links));
        }

        // Vector segments: lanes coming in from above either pass through
        // or end at this commit; parents leave towards the bottom.
        let shown = |a: usize, b: usize| a < max_lanes || b < max_lanes;
        let mut edges = Vec::new();
        for (i, l) in before.iter().enumerate() {
            let Some(l) = l else { continue };
            let (to_lane, to) = if l.ends_here {
                (col, Anchor::Center)
            } else {
                (i, Anchor::Bottom)
            };
            if shown(i, to_lane) {
                edges.push(Edge {
                    from_lane: i,
                    from: Anchor::Top,
                    to_lane,
                    to,
                    color: l.color,
                });
            }
        }
        if !commit.parents.is_empty()
            && lanes.get(col).is_some_and(Option::is_some)
            && shown(col, col)
        {
            edges.push(Edge {
                from_lane: col,
                from: Anchor::Center,
                to_lane: col,
                to: Anchor::Bottom,
                color,
            });
        }
        for &(j, _) in &links {
            if shown(col, j) {
                edges.push(Edge {
                    from_lane: col,
                    from: Anchor::Center,
                    to_lane: j,
                    to: Anchor::Bottom,
                    color: lane_color(j),
                });
            }
        }
        edges.shrink_to_fit();

        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        rows.push(GraphRow {
            lane: col,
            color,
            is_merge: commit.is_merge(),
            width,
            edges,
        });
    }
    rows
}

/// One row as text: lane `i` at column `2 * i`, the column after it holds
/// the horizontal connector towards lane `i + 1`.
fn glyphs(
    commit: &Commit,
    col: usize,
    width: usize,
    before: &[Option<Above>],
    converging: &[usize],
    links: &[(usize, bool)],
) -> String {
    let mut cells = vec![' '; width * 2];
    for (i, l) in before.iter().enumerate() {
        if l.is_some() {
            cells[2 * i] = '│';
        }
    }
    let connect = |cells: &mut Vec<char>, target: usize, end_glyph: char| {
        let (lo, hi) = if target < col {
            (target, col)
        } else {
            (col, target)
        };
        for k in lo..hi {
            cells[2 * k + 1] = '─';
            if k > lo {
                cells[2 * k] = if cells[2 * k] == '│' { '┼' } else { '─' };
            }
        }
        cells[2 * target] = end_glyph;
    };
    for &i in converging {
        connect(&mut cells, i, if i > col { '╯' } else { '╰' });
    }
    for &(j, opened) in links {
        let glyph = match (opened, j > col) {
            (true, true) => '╮',
            (true, false) => '╭',
            (false, true) => '┤',
            (false, false) => '├',
        };
        connect(&mut cells, j, glyph);
    }
    cells[2 * col] = if commit.is_merge() { '◉' } else { '●' };
    cells.into_iter().collect::<String>().trim_end().to_owned()
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
        layout_text(commits, 6)
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
            usize::MAX,
        );
        // The merge continues lane 0 and opens lane 1 in the next color.
        let colors: Vec<_> = rows[0].edges.iter().map(|e| e.color).collect();
        assert_eq!(colors, [0, 1]);
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
            usize::MAX,
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

    #[test]
    fn segments_past_max_lanes_are_left_out() {
        // Four tips side by side, all merging into the root.
        let commits = [
            commit(6, &[1]),
            commit(5, &[1]),
            commit(4, &[1]),
            commit(3, &[1]),
            commit(1, &[]),
        ];
        let all = layout(&commits, 6, usize::MAX);
        let kept = layout(&commits, 6, 2);
        assert!(
            all.iter()
                .any(|r| r.edges.iter().any(|e| e.from_lane >= 2 && e.to_lane >= 2))
        );
        for (a, k) in all.iter().zip(&kept) {
            let shown: Vec<_> = a
                .edges
                .iter()
                .filter(|e| e.from_lane < 2 || e.to_lane < 2)
                .copied()
                .collect();
            // Only hidden segments go: one from lane 3 into lane 0 stays.
            assert_eq!(k.edges, shown);
            assert_eq!((k.lane, k.width), (a.lane, a.width));
        }
        assert!(
            kept[4]
                .edges
                .iter()
                .any(|e| e.from_lane == 3 && e.to_lane == 0)
        );
    }

    #[test]
    fn a_layout_continues_where_the_last_page_ended() {
        let commits = [
            commit(9, &[8, 5]),
            commit(8, &[7]),
            commit(6, &[4]),
            commit(7, &[4, 3]),
            commit(5, &[3]),
            commit(4, &[2]),
            commit(3, &[2]),
            commit(2, &[1]),
            commit(1, &[]),
        ];
        let whole = layout(&commits, 3, 2);
        for split in 0..=commits.len() {
            let mut l = Layout::new(3, 2);
            let mut rows = l.rows(&commits[..split]);
            rows.extend(l.rows(&commits[split..]));
            assert_eq!(rows, whole, "split at {split}");
        }
    }
}
