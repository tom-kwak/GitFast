//! The one text matcher of gf's filters: History's Find, the open window
//! (⌘O) and the tab list (⌘P).
//!
//! A match is the typed text as it is, or pieces of words in order
//! (`dvm` for `diff_view/mod.rs`, `mv` for `MainView`). Letters picked
//! from anywhere do not match: `test` would find most long paths in the
//! t, e, s, t of their folder names.

/// A score above any [`score`]: callers add it to rank one kind of match
/// (the name) above another (the path).
pub const RANK: i32 = 100_000;

/// The typed text, lowercase and without spaces, for [`score`].
pub fn needle(query: &str) -> Vec<char> {
    query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(lower)
        .collect()
}

/// [`score`] for a query typed as is.
pub fn fuzzy(query: &str, hay: &str) -> Option<(i32, Vec<usize>)> {
    score(&needle(query), hay)
}

pub(crate) fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn boundary(hay: &[char], at: usize) -> bool {
    at == 0 || matches!(hay[at - 1], '/' | '\\' | '-' | '_' | '.' | ' ')
}

/// Scores `hay` (a name or path) against `needle` (from [`needle`]) and
/// returns the character indices to highlight; `None` when it does not
/// match.
///
/// A contiguous match wins over a scattered one, and the file name over
/// the folders: `core` marks `core` in `src/core.rs`, not the `c` of `src`.
/// Scattered matches try every start and keep the best-scoring one, rather
/// than the first letters found from the left.
pub fn score(needle: &[char], hay: &str) -> Option<(i32, Vec<usize>)> {
    let hay: Vec<char> = hay.chars().collect();
    let low: Vec<char> = hay.iter().copied().map(lower).collect();
    let n = needle.len();
    if n == 0 || n > low.len() {
        return None;
    }
    let name_start = hay
        .iter()
        .rposition(|&c| c == '/' || c == '\\')
        .map_or(0, |i| i + 1);
    let in_name = |at: usize| if at >= name_start { 20 } else { 0 };

    // Contiguous: the best occurrence (in the name, at a word start, rightmost).
    let substring = (0..=low.len() - n)
        .filter(|&at| low[at..at + n] == *needle)
        .map(|at| {
            let score = 10_000 + in_name(at) + if boundary(&hay, at) { 10 } else { 0 };
            (score, at)
        })
        .max_by_key(|&(score, at)| (score, at));
    if let Some((score, at)) = substring {
        let score = score - i32::try_from(low.len()).unwrap_or(i32::MAX).min(200) / 20;
        return Some((score, (at..at + n).collect()));
    }

    // Scattered: pieces of words, like `dvm` for `diff_view/mod.rs` or
    // `mv` for `MainView`. A letter either follows the previous one or
    // starts a word; letters picked from anywhere (`test` in
    // `Cargo.toml`'s t, e, s, t) would match nearly every long path.
    //
    // best[i]: the best score with the current needle letter at `i`, and
    // where the previous letter was (to read the marks back).
    let starts: Vec<bool> = (0..hay.len()).map(|i| word_start(&hay, i)).collect();
    let gain = |at: usize, joined: bool| {
        1 + if joined { 5 } else { 0 }
            + if starts[at] { 8 } else { 0 }
            + if at >= name_start { 3 } else { 0 }
    };
    let len = low.len();
    let mut rows: Vec<Vec<Option<(i32, usize)>>> = Vec::with_capacity(n);
    let first: Vec<_> = (0..len)
        .map(|i| (low[i] == needle[0] && starts[i]).then(|| (gain(i, false), i)))
        .collect();
    rows.push(first);
    for &c in &needle[1..] {
        let prev = rows.last().expect("first row");
        // The best earlier letter before each position (for a word start).
        let mut before: Option<(i32, usize)> = None;
        let mut row = vec![None; len];
        for i in 0..len {
            if low[i] == c {
                let joined = i
                    .checked_sub(1)
                    .and_then(|j| prev[j].map(|(s, _)| (s + gain(i, true), j)));
                let jumped = before
                    .filter(|_| starts[i])
                    .map(|(s, j)| (s + gain(i, false), j));
                row[i] = match (joined, jumped) {
                    (Some(a), Some(b)) => Some(if b.0 > a.0 { b } else { a }),
                    (a, b) => a.or(b),
                };
            }
            if let Some((s, _)) = prev[i]
                && before.is_none_or(|(b, _)| s > b)
            {
                before = Some((s, i));
            }
        }
        rows.push(row);
    }
    let (mut at, score) = rows
        .last()
        .expect("a row per letter")
        .iter()
        .enumerate()
        .filter_map(|(i, cell)| cell.map(|(s, _)| (i, s)))
        .max_by_key(|&(i, s)| (s, i))?;
    let mut marks = vec![0; n];
    for k in (0..n).rev() {
        marks[k] = at;
        if let Some((_, from)) = rows[k][at] {
            at = from;
        }
    }
    Some((score, marks))
}

/// A word starts at `at`: the path's start, after a separator, a lower to
/// upper case change (`MainView`) or a letter to digit change (`v2`).
fn word_start(hay: &[char], at: usize) -> bool {
    boundary(hay, at)
        || (hay[at - 1].is_lowercase() && hay[at].is_uppercase())
        || (hay[at - 1].is_ascii_digit() != hay[at].is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(query: &str, hay: &str) -> Option<String> {
        let chars: Vec<char> = hay.chars().collect();
        fuzzy(query, hay).map(|(_, marks)| marks.iter().map(|&i| chars[i]).collect())
    }

    #[test]
    fn the_text_as_typed_or_pieces_of_words() {
        assert_eq!(marked("desk", "github-desktop").as_deref(), Some("desk"));
        assert_eq!(marked("gd", "github-desktop").as_deref(), Some("gd"));
        assert_eq!(marked("mv", "src/App/MainView.rs").as_deref(), Some("MV"));
        assert_eq!(
            marked("app main", "src/App/MainView.rs").as_deref(),
            Some("AppMain")
        );
        assert_eq!(marked("gfa", r"C:\code\gf-app").as_deref(), Some("gfa"));
        assert_eq!(marked("v2", "api-v2").as_deref(), Some("v2"));
    }

    #[test]
    fn letters_picked_from_anywhere_are_no_match() {
        assert_eq!(marked("test", "crates/gf-app/Cargo.toml"), None);
        assert_eq!(marked("gd", "xgxxxxxd"), None);
        assert_eq!(
            marked("hub", "github"),
            Some("hub".into()),
            "inside a word, as typed"
        );
        assert_eq!(marked("ghb", "github"), None);
    }

    #[test]
    fn as_typed_ranks_above_pieces_and_the_name_above_folders() {
        let typed = fuzzy("gf", "tools/gf").expect("typed").0;
        let pieces = fuzzy("gf", "git-flow").expect("pieces").0;
        assert!(typed > pieces);
        let name = fuzzy("core", "src/core.rs").expect("name").0;
        let folder = fuzzy("core", "core/src.rs").expect("folder").0;
        assert!(name > folder);
        assert!(pieces + RANK > typed, "RANK puts one kind of match first");
    }
}
