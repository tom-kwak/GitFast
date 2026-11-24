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
//! A [`Layout::with_cut`] layout does not draw lines to parents far below:
//! a topic branch forked off a commit a thousand rows down would otherwise
//! keep an empty lane open for all those rows, and a history with many such
//! topics (git.git) grows hundreds of lanes wide. The row gets an arrow down
//! instead ([`GraphRow::cut_parents`]), and the parent an arrow up
//! ([`GraphRow::cut_children`]).
//!
//! Input must be in an order where children come before parents
//! (`git log --topo-order` / `--date-order`). Pure and UI-independent.

use gf_core::model::{Commit, Oid};
use std::collections::HashMap;

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
    /// Cut lines, each ending in an arrow head at its `to` end: down from
    /// the dot towards a parent far below, up from the dot towards children
    /// far above, or a lane carried over from the previous page that ends
    /// here because its commit turned out to be far below.
    pub arrows: Vec<Edge>,
    /// Indices into the commit's parents whose lines were cut.
    pub cut_parents: Vec<usize>,
    /// Children above whose lines to this commit were cut.
    pub cut_children: usize,
}

#[derive(Debug, Clone)]
struct Lane {
    expects: Oid,
    color: usize,
    /// Row (counted over every page) the lane started waiting at.
    opened: usize,
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
    Layout::new(colors, usize::MAX).texts(commits)
}

/// The lanes open after the rows laid out so far. [`Layout::rows`] lays out
/// the commits that come next in the same order — the next page of a
/// history — continuing the lanes of the first ones.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    lanes: Vec<Option<Lane>>,
    next_color: usize,
    colors: usize,
    max_lanes: usize,
    /// Lines to parents more than this many rows below are cut.
    cut: Option<usize>,
    /// Rows laid out so far.
    row: usize,
    /// Commits not laid out yet, and how many children cut their lines to
    /// them.
    cut_targets: HashMap<Oid, usize>,
}

impl Layout {
    /// No rows yet; `colors` and `max_lanes` as for [`layout`].
    pub fn new(colors: usize, max_lanes: usize) -> Self {
        Self {
            colors: colors.max(1),
            max_lanes,
            ..Self::default()
        }
    }

    /// Cuts lines to parents more than `rows` rows below (see the module
    /// docs).
    ///
    /// A parent past the end of the commits given to [`Self::rows`] is
    /// taken as far when at least `rows` commits follow; nearer the end the
    /// lane stays open, and the next page ends it in its first row with an
    /// arrow if the parent turns out to be far after all.
    #[must_use]
    pub fn with_cut(mut self, rows: usize) -> Self {
        self.cut = Some(rows);
        self
    }

    /// Rows of `commits`, continuing from the rows laid out before.
    pub fn rows(&mut self, commits: &[Commit]) -> Vec<GraphRow> {
        self.walk(commits, None)
    }

    /// Rows of `commits` as glyphs (see [`layout_text`]).
    pub fn texts(&mut self, commits: &[Commit]) -> Vec<String> {
        let mut texts = Vec::with_capacity(commits.len());
        self.walk(commits, Some(&mut texts));
        texts
    }

    /// A new lane waiting for `expects` in the first free slot.
    fn open(&mut self, expects: Oid, color: usize, at: usize) -> usize {
        let slot = self
            .lanes
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.lanes.len());
        let lane = Lane {
            expects,
            color,
            opened: at,
        };
        if slot == self.lanes.len() {
            self.lanes.push(Some(lane));
        } else {
            self.lanes[slot] = Some(lane);
        }
        slot
    }

    fn new_color(&mut self) -> usize {
        let c = self.next_color % self.colors.max(1);
        self.next_color += 1;
        c
    }

    /// Ends lanes carried over from the last page whose commit is more than
    /// `cut` rows below where they started. Returns them as (lane, color).
    fn cut_carried(&mut self, page: &HashMap<&Oid, usize>, len: usize) -> Vec<(usize, usize)> {
        let (Some(cut), true) = (self.cut, len > 0) else {
            return Vec::new();
        };
        let mut ended = Vec::new();
        for (i, slot) in self.lanes.iter_mut().enumerate() {
            let Some(lane) = slot else { continue };
            // Not on this page: at least past its end.
            let at = self.row + page.get(&lane.expects).copied().unwrap_or(len);
            if at.saturating_sub(lane.opened) > cut {
                *self.cut_targets.entry(lane.expects.clone()).or_default() += 1;
                ended.push((i, lane.color));
                *slot = None;
            }
        }
        ended
    }

    #[allow(clippy::too_many_lines)]
    fn walk(&mut self, commits: &[Commit], mut texts: Option<&mut Vec<String>>) -> Vec<GraphRow> {
        let max_lanes = self.max_lanes;
        let page: HashMap<&Oid, usize> = if self.cut.is_some() {
            commits
                .iter()
                .enumerate()
                .map(|(i, c)| (&c.id, i))
                .collect()
        } else {
            HashMap::new()
        };
        let len = commits.len();
        // Is `parent` of the commit at `i` too far below to draw a line to?
        let far = |cut: Option<usize>, parent: &Oid, i: usize| -> bool {
            let Some(cut) = cut else { return false };
            match page.get(parent) {
                Some(&p) => p.saturating_sub(i) > cut,
                // Past the end of this page.
                None => len - i > cut,
            }
        };
        let mut carried = self.cut_carried(&page, len);
        let mut rows = Vec::with_capacity(len);
        let mut before: Vec<Option<Above>> = Vec::new();

        for (i, commit) in commits.iter().enumerate() {
            let at = self.row + i;
            before.clear();
            before.extend(self.lanes.iter().map(|l| {
                l.as_ref().map(|l| Above {
                    ends_here: l.expects == commit.id,
                    color: l.color,
                })
            }));
            let cut_children = self.cut_targets.remove(&commit.id).unwrap_or(0);

            // Where this commit is drawn: the lane already waiting for it, or
            // a free slot (a branch tip, or the far end of cut lines).
            let waited = before.iter().position(|l| l.is_some_and(|l| l.ends_here));
            let col = match waited {
                Some(c) => c,
                None => {
                    let color = self.new_color();
                    self.open(commit.id.clone(), color, at)
                }
            };
            let color = self.lanes[col].as_ref().map_or(0, |l| l.color);

            // Other lanes that were also waiting for this commit converge here.
            let converging: Vec<usize> = before
                .iter()
                .enumerate()
                .filter(|(i, l)| *i != col && l.is_some_and(|l| l.ends_here))
                .map(|(i, _)| i)
                .collect();
            for &i in &converging {
                self.lanes[i] = None;
            }

            // Parents: the first continues this lane, others join or open
            // lanes; far ones are cut.
            // (target lane, opened new lane?)
            let mut links: Vec<(usize, bool)> = Vec::new();
            let mut cut_parents = Vec::new();
            self.lanes[col] = None;
            for (n, parent) in commit.parents.iter().enumerate() {
                let joins = (n > 0)
                    .then(|| {
                        self.lanes
                            .iter()
                            .position(|l| l.as_ref().is_some_and(|l| &l.expects == parent))
                    })
                    .flatten();
                if let Some(j) = joins {
                    links.push((j, false));
                } else if far(self.cut, parent, i) {
                    *self.cut_targets.entry(parent.clone()).or_default() += 1;
                    cut_parents.push(n);
                } else if n == 0 {
                    // The first parent always continues this lane, even if
                    // another lane waits for it too; they converge at the
                    // parent's row.
                    self.lanes[col] = Some(Lane {
                        expects: parent.clone(),
                        color,
                        opened: at,
                    });
                } else {
                    let c = self.new_color();
                    links.push((self.open(parent.clone(), c, at), true));
                }
            }

            let lanes = &self.lanes;
            let width = carried
                .iter()
                .map(|&(l, _)| l + 1)
                .fold(before.len().max(lanes.len()).max(col + 1), usize::max);
            let lane_color = |i: usize| {
                lanes
                    .get(i)
                    .and_then(|l| l.as_ref())
                    .map(|l| l.color)
                    .or_else(|| before.get(i).copied().flatten().map(|l| l.color))
                    .unwrap_or(color)
            };
            let carried_here = std::mem::take(&mut carried);
            if let Some(texts) = texts.as_deref_mut() {
                let ends: Vec<usize> = carried_here.iter().map(|&(l, _)| l).collect();
                texts.push(glyphs(
                    commit,
                    col,
                    width,
                    &before,
                    &converging,
                    &links,
                    &ends,
                ));
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
            if lanes.get(col).is_some_and(Option::is_some) && shown(col, col) {
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

            let mut arrows = Vec::new();
            for &(l, c) in &carried_here {
                arrows.push(Edge {
                    from_lane: l,
                    from: Anchor::Top,
                    to_lane: l,
                    to: Anchor::Center,
                    color: c,
                });
            }
            if cut_children > 0 && waited.is_none() {
                arrows.push(Edge {
                    from_lane: col,
                    from: Anchor::Center,
                    to_lane: col,
                    to: Anchor::Top,
                    color,
                });
            }
            for &n in &cut_parents {
                // The first parent straight down, others off to the side.
                let to_lane = if n == 0 { col } else { col + 1 };
                arrows.push(Edge {
                    from_lane: col,
                    from: Anchor::Center,
                    to_lane,
                    to: Anchor::Bottom,
                    color,
                });
            }
            arrows.retain(|a| shown(a.from_lane, a.to_lane));

            while self.lanes.last().is_some_and(Option::is_none) {
                self.lanes.pop();
            }
            rows.push(GraphRow {
                lane: col,
                color,
                is_merge: commit.is_merge(),
                width,
                edges,
                arrows,
                cut_parents,
                cut_children,
            });
        }
        self.row += len;
        rows
    }
}

/// One row as text: lane `i` at column `2 * i`, the column after it holds
/// the horizontal connector towards lane `i + 1`. Lanes in `ends` end here
/// with an arrow (`↓`).
fn glyphs(
    commit: &Commit,
    col: usize,
    width: usize,
    before: &[Option<Above>],
    converging: &[usize],
    links: &[(usize, bool)],
    ends: &[usize],
) -> String {
    let width = ends.iter().map(|&l| l + 1).fold(width, usize::max);
    let mut cells = vec![' '; width * 2];
    for (i, l) in before.iter().enumerate() {
        if l.is_some() {
            cells[2 * i] = '│';
        }
    }
    for &l in ends {
        cells[2 * l] = '↓';
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

    /// A merge of a one-commit topic forked off the root, `main` commits
    /// below: `M(main, t)`, `t(root)`, `main_n .. main_1`, `root`.
    fn long_topic(main: u8) -> Vec<Commit> {
        let mut commits = vec![commit(200, &[100 + main, 150]), commit(150, &[1])];
        for n in (1..=main).rev() {
            let parent = if n == 1 { 1 } else { 100 + n - 1 };
            commits.push(commit(100 + n, &[parent]));
        }
        commits.push(commit(1, &[]));
        commits
    }

    #[test]
    fn lines_to_far_parents_are_cut() {
        let commits = long_topic(10);
        let open = layout(&commits, 6, usize::MAX);
        // Uncut, the topic's lane runs down to the root.
        assert!(open[2..].iter().all(|r| r.width == 2));

        let rows = Layout::new(6, usize::MAX).with_cut(3).rows(&commits);
        let topic = &rows[1];
        assert_eq!((topic.lane, topic.cut_parents.as_slice()), (1, &[0][..]));
        use Anchor::{Bottom, Center};
        assert_eq!(
            topic
                .arrows
                .iter()
                .map(|a| (a.from_lane, a.from, a.to_lane, a.to))
                .collect::<Vec<_>>(),
            [(1, Center, 1, Bottom)]
        );
        // Nothing continues below the topic commit.
        assert!(rows[2..].iter().all(|r| r.width == 1));
        let root = rows.last().expect("root");
        assert_eq!(root.cut_children, 1);
        // The main lane arrives, so no arrow up is drawn.
        assert!(root.arrows.is_empty());
        // Near parents keep their lines.
        assert!(rows[0].cut_parents.is_empty());
    }

    #[test]
    fn a_commit_reached_only_by_cut_lines_gets_an_arrow_up() {
        // Two tips far above their shared parent: 9 and 8 -> ... -> 1.
        let mut commits = vec![commit(9, &[1])];
        for n in (2..=8).rev() {
            commits.push(commit(n, &[n - 1]));
        }
        commits.push(commit(1, &[]));
        let rows = Layout::new(6, usize::MAX).with_cut(3).rows(&commits);
        assert_eq!(rows[0].cut_parents, [0]);
        let root = rows.last().expect("root");
        assert_eq!(root.cut_children, 1);
        assert!(root.arrows.is_empty(), "the 2 -> 1 lane arrives");

        let rows = Layout::new(6, usize::MAX).with_cut(3).rows(&[
            commit(9, &[1]),
            commit(5, &[]),
            commit(4, &[]),
            commit(3, &[]),
            commit(2, &[]),
            commit(1, &[]),
        ]);
        let root = rows.last().expect("root");
        assert_eq!(root.cut_children, 1);
        assert_eq!(
            root.arrows
                .iter()
                .map(|a| (a.from, a.to))
                .collect::<Vec<_>>(),
            [(Anchor::Center, Anchor::Top)]
        );
    }

    #[test]
    fn a_lane_near_the_end_of_a_page_is_cut_on_the_next_page() {
        let commits = long_topic(10);
        let mut l = Layout::new(6, usize::MAX).with_cut(3);
        // The topic commit is last on the page: its parent may be near.
        let first = l.texts(&commits[..2]);
        assert_eq!(first, ["◉─╮", "│ ●"]);
        let rest = l.texts(&commits[2..]);
        assert_eq!(rest[0], "● ↓");
        assert!(rest[1..].iter().all(|t| t == "●"));
        let mut l = Layout::new(6, usize::MAX).with_cut(3);
        l.rows(&commits[..2]);
        let rows = l.rows(&commits[2..]);
        assert_eq!(rows[0].arrows.len(), 1);
        assert_eq!(rows.last().expect("root").cut_children, 1);
    }

    #[test]
    fn without_a_cut_nothing_is_cut() {
        let rows = layout(&long_topic(10), 6, usize::MAX);
        assert!(
            rows.iter()
                .all(|r| r.arrows.is_empty() && r.cut_parents.is_empty() && r.cut_children == 0)
        );
    }
}
