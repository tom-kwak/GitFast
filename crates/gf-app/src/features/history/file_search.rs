//! Candidates for History's Find File: the paths in one commit's tree, the
//! matcher that ranks and highlights them, and the bookkeeping around it.
//!
//! The list is read once per commit (cached per tab) and filtered on a
//! worker, one job at a time, so typing never calls git or blocks the UI;
//! results for a closed search or an older query are dropped.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gf_core::model::Oid;

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    key: String,
}

#[derive(Debug, Clone)]
pub struct Index {
    candidates: Vec<Candidate>,
}

impl Index {
    /// Builds display and search keys. Call from a blocking worker.
    pub fn prepare(paths: Vec<PathBuf>) -> Self {
        Self {
            candidates: paths
                .into_iter()
                .map(|path| Candidate {
                    key: path.to_string_lossy().into_owned(),
                    path,
                })
                .collect(),
        }
    }

    pub fn path(&self, index: usize) -> Option<&Path> {
        self.candidates
            .get(index)
            .map(|candidate| candidate.path.as_path())
    }

    pub fn label(&self, index: usize) -> Option<&str> {
        self.candidates
            .get(index)
            .map(|candidate| candidate.key.as_str())
    }

    pub fn filter(&self, query: &str) -> Vec<Hit> {
        if query.trim().is_empty() {
            return (0..self.candidates.len())
                .map(|candidate| Hit {
                    candidate,
                    marks: Vec::new(),
                })
                .collect();
        }

        let needle: Vec<char> = query
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(lower)
            .collect();
        let mut scored: Vec<_> = self
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(candidate, entry)| {
                path_match(&needle, &entry.key)
                    .map(|(score, marks)| (score, Hit { candidate, marks }))
            })
            .collect();
        scored.sort_by_key(|(score, _)| Reverse(*score));
        scored.into_iter().map(|(_, hit)| hit).collect()
    }
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn boundary(hay: &[char], at: usize) -> bool {
    at == 0 || matches!(hay[at - 1], '/' | '-' | '_' | '.' | ' ')
}

/// Scores `hay` (a repository path) against `needle` (lowercase, no
/// spaces) and returns the character indices to highlight.
///
/// A contiguous match wins over a scattered one, and the file name over
/// the folders: `core` marks `core` in `src/core.rs`, not the `c` of `src`.
/// Scattered matches try every start and keep the best-scoring one, rather
/// than the first letters found from the left.
fn path_match(needle: &[char], hay: &str) -> Option<(i32, Vec<usize>)> {
    let hay: Vec<char> = hay.chars().collect();
    let low: Vec<char> = hay.iter().copied().map(lower).collect();
    let n = needle.len();
    if n == 0 || n > low.len() {
        return None;
    }
    let name_start = hay.iter().rposition(|&c| c == '/').map_or(0, |i| i + 1);
    let in_name = |at: usize| if at >= name_start { 20 } else { 0 };

    // Contiguous: the best occurrence (in the name, at a word start, rightmost).
    let substring = (0..=low.len() - n)
        .filter(|&at| low[at..at + n] == *needle)
        .map(|at| {
            let score = 1000 + in_name(at) + if boundary(&hay, at) { 10 } else { 0 };
            (score, at)
        })
        .max_by_key(|&(score, at)| (score, at));
    if let Some((score, at)) = substring {
        let score = score - i32::try_from(low.len()).unwrap_or(i32::MAX).min(200) / 20;
        return Some((score, (at..at + n).collect()));
    }

    // Scattered: greedy from each possible first letter, best score wins.
    let mut best: Option<(i32, Vec<usize>)> = None;
    for start in (0..low.len()).filter(|&i| low[i] == needle[0]) {
        let mut marks = Vec::with_capacity(n);
        let mut score = 0;
        let mut from = start;
        for &c in needle {
            let Some(at) = (from..low.len()).find(|&i| low[i] == c) else {
                marks.clear();
                break;
            };
            score += 1;
            if marks.last().is_some_and(|&last: &usize| last + 1 == at) {
                score += 5;
            }
            if boundary(&hay, at) {
                score += 8;
            }
            if at >= name_start {
                score += 3;
            }
            if !marks.is_empty() {
                score -= i32::try_from(at - from).unwrap_or(i32::MAX).min(10) / 4;
            }
            marks.push(at);
            from = at + 1;
        }
        if marks.len() != n {
            // A later start cannot match either: it has fewer letters left.
            break;
        }
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, marks));
        }
    }
    best
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub candidate: usize,
    pub marks: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct LoadRequest {
    pub request: u64,
    pub oid: Oid,
}

#[derive(Debug, Clone)]
pub struct FilterJob {
    pub request: u64,
    pub generation: u64,
    pub oid: Oid,
    pub index: Arc<Index>,
    pub query: String,
}

#[derive(Debug, Clone)]
pub enum Work {
    Load(LoadRequest),
    Filter(FilterJob),
}

#[derive(Debug, Clone, Default)]
enum Status {
    #[default]
    Closed,
    NoCommit,
    Loading,
    Filtering,
    Ready,
    Failed(String),
}

#[derive(Debug, Default)]
pub struct State {
    open: bool,
    oid: Option<Oid>,
    query: String,
    query_generation: u64,
    cache: Option<(Oid, Arc<Index>)>,
    next_load: u64,
    pending_load: Option<(u64, Oid)>,
    next_filter: u64,
    in_flight_filter: Option<(u64, u64)>,
    hits: Arc<Vec<Hit>>,
    status: Status,
}

impl State {
    pub fn open(&mut self, oid: Option<Oid>) -> Option<Work> {
        self.open = true;
        self.oid = oid.clone();
        self.query.clear();
        self.query_generation = self.query_generation.wrapping_add(1);
        self.pending_load = None;
        self.hits = Arc::default();

        let Some(oid) = oid else {
            self.status = Status::NoCommit;
            return None;
        };
        if self
            .cache
            .as_ref()
            .is_some_and(|(cached, _)| cached == &oid)
        {
            self.status = Status::Filtering;
            return self.start_filter().map(Work::Filter);
        }

        self.next_load = self.next_load.wrapping_add(1);
        let request = self.next_load;
        self.pending_load = Some((request, oid.clone()));
        self.status = Status::Loading;
        Some(Work::Load(LoadRequest { request, oid }))
    }

    pub fn close(&mut self) {
        self.open = false;
        self.oid = None;
        self.pending_load = None;
        self.query.clear();
        self.query_generation = self.query_generation.wrapping_add(1);
        self.hits = Arc::default();
        self.status = Status::Closed;
    }

    pub fn query_changed(&mut self, query: String) -> Option<FilterJob> {
        if !self.open {
            return None;
        }
        self.query = query;
        self.query_generation = self.query_generation.wrapping_add(1);
        // The previous rows stay until the new ones arrive (no flashing).
        if self.has_current_index() {
            self.status = Status::Filtering;
            self.start_filter()
        } else {
            None
        }
    }

    pub fn paths_loaded(
        &mut self,
        request: u64,
        oid: &Oid,
        result: Result<Arc<Index>, String>,
    ) -> Option<FilterJob> {
        if !self.open
            || self.pending_load.as_ref() != Some(&(request, oid.clone()))
            || self.oid.as_ref() != Some(oid)
        {
            return None;
        }
        self.pending_load = None;
        match result {
            Ok(index) => self.cache = Some((oid.clone(), index)),
            Err(error) => {
                self.status = Status::Failed(error);
                return None;
            }
        }
        self.status = Status::Filtering;
        self.start_filter()
    }

    pub fn filter_finished(
        &mut self,
        request: u64,
        generation: u64,
        oid: &Oid,
        hits: Vec<Hit>,
    ) -> FilterCompletion {
        if self.in_flight_filter != Some((request, generation)) {
            return FilterCompletion::default();
        }
        self.in_flight_filter = None;

        let accepted = self.open
            && self.oid.as_ref() == Some(oid)
            && self.query_generation == generation
            && self.has_current_index();
        if accepted {
            self.hits = Arc::new(hits);
            self.status = Status::Ready;
        } else if self.open && self.has_current_index() {
            self.status = Status::Filtering;
        }

        let result_count = if accepted { self.hits.len() } else { 0 };
        let next = (!accepted && self.open)
            .then(|| self.start_filter())
            .flatten();
        FilterCompletion {
            accepted,
            result_count,
            next,
        }
    }

    pub fn status_text(&self) -> Option<&str> {
        match &self.status {
            Status::Closed | Status::Ready => None,
            Status::NoCommit => Some("No commit exists in this history"),
            Status::Loading => Some("Loading files…"),
            Status::Filtering => Some("Filtering files…"),
            Status::Failed(error) => Some(error),
        }
    }

    pub fn label_for_hit(&self, row: usize) -> Option<&str> {
        let hit = self.hits.get(row)?;
        self.cache.as_ref()?.1.label(hit.candidate)
    }

    pub fn marks_for_hit(&self, row: usize) -> Option<&[usize]> {
        self.hits.get(row).map(|hit| hit.marks.as_slice())
    }

    pub fn path_for_hit(&self, row: usize) -> Option<&Path> {
        let hit = self.hits.get(row)?;
        self.cache.as_ref()?.1.path(hit.candidate)
    }

    pub fn current_oid(&self) -> Option<&Oid> {
        self.oid.as_ref()
    }

    fn has_current_index(&self) -> bool {
        self.oid
            .as_ref()
            .is_some_and(|oid| self.cache.as_ref().is_some_and(|(cached, _)| cached == oid))
    }

    fn start_filter(&mut self) -> Option<FilterJob> {
        if !self.open || self.in_flight_filter.is_some() {
            return None;
        }
        let oid = self.oid.clone()?;
        let (cached_oid, index) = self.cache.as_ref()?;
        if cached_oid != &oid {
            return None;
        }

        self.next_filter = self.next_filter.wrapping_add(1);
        let request = self.next_filter;
        let generation = self.query_generation;
        self.in_flight_filter = Some((request, generation));
        Some(FilterJob {
            request,
            generation,
            oid,
            index: index.clone(),
            query: self.query.clone(),
        })
    }
}

#[derive(Debug, Default)]
pub struct FilterCompletion {
    pub accepted: bool,
    pub result_count: usize,
    pub next: Option<FilterJob>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(value: char) -> Oid {
        value.to_string().repeat(40).parse().expect("oid")
    }

    #[test]
    fn same_name_files_are_matched_by_their_full_paths() {
        let index = Index::prepare(vec![
            PathBuf::from("src/lib.rs"),
            PathBuf::from("tests/lib.rs"),
        ]);
        let hits = index.filter("tests/lib");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            index.path(hits[0].candidate),
            Some(Path::new("tests/lib.rs"))
        );
    }

    fn marked(index: &Index, hit: &Hit) -> String {
        let label: Vec<char> = index.label(hit.candidate).expect("label").chars().collect();
        hit.marks.iter().map(|&i| label[i]).collect()
    }

    #[test]
    fn highlights_the_file_name_not_the_first_letters_from_the_left() {
        let index = Index::prepare(vec![PathBuf::from("src/core.rs")]);
        let hits = index.filter("core");
        assert_eq!(hits[0].marks, vec![4, 5, 6, 7]);
        assert_eq!(marked(&index, &hits[0]), "core");
    }

    #[test]
    fn contiguous_and_file_name_matches_rank_first() {
        let index = Index::prepare(vec![
            PathBuf::from("crates/order/x.rs"),
            PathBuf::from("lib/core/mod.rs"),
            PathBuf::from("src/core.rs"),
        ]);
        let hits = index.filter("core");
        let order: Vec<_> = hits
            .iter()
            .map(|h| index.label(h.candidate).expect("label"))
            .collect();
        assert_eq!(order[0], "src/core.rs");
        assert_eq!(order[1], "lib/core/mod.rs");
        for hit in &hits[..2] {
            assert_eq!(marked(&index, hit), "core");
        }
    }

    #[test]
    fn scattered_matches_pick_the_best_alignment_and_ignore_case() {
        let index = Index::prepare(vec![PathBuf::from("src/App/MainView.rs")]);
        let hits = index.filter("mv");
        assert_eq!(hits.len(), 1);
        assert_eq!(marked(&index, &hits[0]), "MV");
        let hits = index.filter("app main");
        assert_eq!(marked(&index, &hits[0]), "AppMain");
    }

    #[test]
    fn non_ascii_names_keep_marks_on_characters() {
        let index = Index::prepare(vec![PathBuf::from("문서/설계.md")]);
        let hits = index.filter("설계");
        assert_eq!(marked(&index, &hits[0]), "설계");
    }

    #[test]
    fn the_previous_rows_stay_until_the_new_query_finishes() {
        let mut search = State::default();
        let load = match search.open(Some(oid('e'))).expect("load") {
            Work::Load(load) => load,
            Work::Filter(_) => panic!("cache is empty"),
        };
        let index = Arc::new(Index::prepare(vec![
            PathBuf::from("src/lib.rs"),
            PathBuf::from("src/main.rs"),
        ]));
        let first = search
            .paths_loaded(load.request, &load.oid, Ok(index))
            .expect("initial filter");
        let done = search.filter_finished(
            first.request,
            first.generation,
            &first.oid,
            first.index.filter(&first.query),
        );
        assert!(done.accepted);
        assert_eq!(search.label_for_hit(1), Some("src/main.rs"));

        // Typing starts a new filter: nothing on screen disappears meanwhile.
        let job = search.query_changed("main".into()).expect("filter");
        assert_eq!(search.label_for_hit(0), Some("src/lib.rs"));
        assert_eq!(search.label_for_hit(1), Some("src/main.rs"));

        let done = search.filter_finished(
            job.request,
            job.generation,
            &job.oid,
            job.index.filter(&job.query),
        );
        assert!(done.accepted);
        assert_eq!(search.label_for_hit(0), Some("src/main.rs"));
        assert_eq!(search.label_for_hit(1), None);
    }

    #[test]
    fn only_latest_query_runs_after_an_in_flight_filter() {
        let mut search = State::default();
        let first = match search.open(Some(oid('a'))).expect("load") {
            Work::Load(load) => load,
            Work::Filter(_) => panic!("cache is empty"),
        };
        let index = Arc::new(Index::prepare(vec![PathBuf::from("src/lib.rs")]));
        let first_filter = search
            .paths_loaded(first.request, &first.oid, Ok(index))
            .expect("initial filter");
        assert!(search.query_changed("src".into()).is_none());
        assert!(search.query_changed("lib".into()).is_none());

        let done = search.filter_finished(
            first_filter.request,
            first_filter.generation,
            &first_filter.oid,
            first_filter.index.filter(&first_filter.query),
        );
        assert!(!done.accepted, "the stale query is never applied");
        let next = done.next.expect("latest query scheduled");
        assert_eq!(next.query, "lib");
        assert_eq!(next.generation, first_filter.generation + 2);
    }

    #[test]
    fn closed_search_ignores_late_path_results_and_keeps_successful_cache() {
        let mut search = State::default();
        let load = match search.open(Some(oid('a'))).expect("load") {
            Work::Load(load) => load,
            Work::Filter(_) => panic!("cache is empty"),
        };
        search.close();
        let index = Arc::new(Index::prepare(vec![PathBuf::from("file.txt")]));
        assert!(
            search
                .paths_loaded(load.request, &load.oid, Ok(index))
                .is_none()
        );
        assert!(
            search.open(Some(oid('a'))).is_some(),
            "stale result was not cached"
        );
    }

    #[test]
    fn a_successful_candidate_list_is_reused_for_the_same_commit() {
        let mut search = State::default();
        let load = match search.open(Some(oid('b'))).expect("load") {
            Work::Load(load) => load,
            Work::Filter(_) => panic!("cache is empty"),
        };
        let index = Arc::new(Index::prepare(vec![PathBuf::from("src/lib.rs")]));
        let filter = search
            .paths_loaded(load.request, &load.oid, Ok(index))
            .expect("initial filter");
        let completion = search.filter_finished(
            filter.request,
            filter.generation,
            &filter.oid,
            filter.index.filter(&filter.query),
        );
        assert!(completion.accepted);
        search.close();

        match search.open(Some(load.oid)).expect("cached filter") {
            Work::Filter(job) => assert_eq!(job.query, ""),
            Work::Load(_) => panic!("same-commit candidates should be cached"),
        }
    }

    #[test]
    fn no_commit_and_load_failure_have_distinct_statuses() {
        let mut search = State::default();
        assert!(search.open(None).is_none());
        assert_eq!(
            search.status_text(),
            Some("No commit exists in this history")
        );

        let load = match search.open(Some(oid('c'))).expect("load") {
            Work::Load(load) => load,
            Work::Filter(_) => panic!("cache is empty"),
        };
        assert!(
            search
                .paths_loaded(load.request, &load.oid, Err("tree lookup failed".into()))
                .is_none()
        );
        assert_eq!(search.status_text(), Some("tree lookup failed"));
    }

    #[test]
    fn a_filter_finishing_after_close_releases_the_single_worker_slot() {
        let mut search = State::default();
        let load = match search.open(Some(oid('d'))).expect("load") {
            Work::Load(load) => load,
            Work::Filter(_) => panic!("cache is empty"),
        };
        let filter = search
            .paths_loaded(
                load.request,
                &load.oid,
                Ok(Arc::new(Index::prepare(vec![PathBuf::from("src/lib.rs")]))),
            )
            .expect("filter");
        search.close();
        let completion = search.filter_finished(
            filter.request,
            filter.generation,
            &filter.oid,
            filter.index.filter(&filter.query),
        );
        assert!(!completion.accepted);
        assert!(completion.next.is_none());

        assert!(matches!(search.open(Some(load.oid)), Some(Work::Filter(_))));
    }
}
