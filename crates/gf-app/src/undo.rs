//! Undo / redo history of one repository (see `gf_git::ops::undo` for how
//! state is captured and put back).
//!
//! Every editing operation gf runs records a [`Step`]: what it touched
//! (its scope) captured before and after. Undo restores `before`, redo
//! restores `after`; each first checks the repository still matches the
//! other side, so later changes are never overwritten without asking.
//!
//! The undo stack is saved in the repository's git dir (`gf/undo.json`)
//! so a discard can still be brought back after gf restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gf_core::model::{StatusEntry, StatusReport};
use gf_git::{Capture, HeadMode, Scope};
use serde::{Deserialize, Serialize};

/// Steps kept per repository.
pub const MAX_STEPS: usize = 50;

/// One undoable operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub label: String,
    pub scope: Scope,
    pub before: Capture,
    pub after: Capture,
    /// How HEAD moves back when the branch stays the same.
    pub mode: HeadMode,
    /// Message of an undone commit, put back in the commit box.
    pub message: Option<String>,
    /// When it ran (unix seconds).
    pub at: u64,
}

impl Step {
    pub fn new(
        label: String,
        scope: Scope,
        before: Capture,
        after: Capture,
        mode: HeadMode,
    ) -> Self {
        Self {
            label,
            scope,
            before,
            after,
            mode,
            message: None,
            at: now(),
        }
    }

    /// Undoing / redoing it moves HEAD (asks first, and is not possible in
    /// the middle of a merge or rebase).
    pub fn moves_head(&self) -> bool {
        self.scope.head && self.before.head != self.after.head
    }

    /// What the repository must look like for `redo` (or undo) to run
    /// without overwriting anything.
    pub fn expected(&self, redo: bool) -> &Capture {
        if redo { &self.before } else { &self.after }
    }

    /// What `redo` (or undo) puts back.
    pub fn target(&self, redo: bool) -> &Capture {
        if redo { &self.after } else { &self.before }
    }

    /// `(branch, from, to)` when `redo` (or undo) moves a branch that
    /// stays checked out (`to` is `None` when it goes back to unborn):
    /// the commits it takes off may already be pushed.
    pub fn branch_move(&self, redo: bool) -> Option<(&str, &str, Option<&str>)> {
        batch_move(std::slice::from_ref(self), redo)
    }
}

/// An operation that stopped on conflicts: its step is recorded once it
/// is continued to the end (and dropped when aborted). Saved with the
/// history, so quitting gf in the middle of a merge keeps it undoable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub label: String,
    pub scope: Scope,
    pub before: Capture,
    pub mode: HeadMode,
}

/// Several undo / redo steps asked at once (from the history list): one
/// confirmation for all of them, then each runs (and is guarded) in turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queue {
    pub redo: bool,
    /// Steps asked for.
    pub total: usize,
    /// Steps still to run after the current one.
    pub left: usize,
    /// The batch was confirmed (or needed no confirmation): the steps
    /// after the first run without asking.
    pub confirmed: bool,
    /// Message of the stash made before the first step, if any.
    pub stashed: Option<String>,
}

impl Queue {
    pub fn new(redo: bool, total: usize) -> Self {
        Self {
            redo,
            total,
            left: total.saturating_sub(1),
            confirmed: false,
            stashed: None,
        }
    }

    /// Steps finished once the current one is.
    pub fn done(&self) -> usize {
        self.total - self.left
    }
}

/// `(branch, from, to)` when `steps` (run in this order, `redo` or undo)
/// move a branch that stays checked out, from the first one that moves
/// HEAD to the last: the commits that leave it may already be pushed.
pub fn batch_move(steps: &[Step], redo: bool) -> Option<(&str, &str, Option<&str>)> {
    let mut moving = steps.iter().filter(|s| s.moves_head());
    let first = moving.next()?;
    let last = moving.next_back().unwrap_or(first);
    let from = first.expected(redo).head.as_ref()?;
    let to = last.target(redo).head.as_ref()?;
    let branch = from.branch.as_deref()?;
    (to.branch.as_deref() == Some(branch)).then_some((
        branch,
        from.oid.as_deref()?,
        to.oid.as_deref(),
    ))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    /// Oldest first; the last one is undone next.
    pub undo: Vec<Step>,
    /// Undone steps, the last one is redone next. Saved too, so an undo
    /// can still be redone after gf restarts (each redo is guarded).
    #[serde(default)]
    pub redo: Vec<Step>,
    /// An operation that stopped on conflicts (see [`Pending`]).
    #[serde(default)]
    pub pending: Option<Pending>,
}

impl History {
    /// A new step: what was undone can no longer be redone.
    pub fn push(&mut self, step: Step) {
        self.redo.clear();
        self.undo.push(step);
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
    }

    /// The next `n` steps `redo` (or undo) would run, in that order.
    pub fn upcoming(&self, redo: bool, n: usize) -> Vec<Step> {
        let side = if redo { &self.redo } else { &self.undo };
        side.iter().rev().take(n).cloned().collect()
    }

    pub fn next(&self, redo: bool) -> Option<&Step> {
        if redo {
            self.redo.last()
        } else {
            self.undo.last()
        }
    }

    /// The step `redo` (or undo) was run for has been applied: move it to
    /// the other stack.
    pub fn applied(&mut self, redo: bool) {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        if let Some(step) = from.pop() {
            to.push(step);
        }
    }

    /// Ids of every saved work tree copy a step can still write back.
    pub fn saved_blobs(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .undo
            .iter()
            .chain(&self.redo)
            .flat_map(|s| s.before.blobs().chain(s.after.blobs()))
            .chain(self.pending.iter().flat_map(|p| p.before.blobs()))
            .map(str::to_owned)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Sets the operation waiting on conflicts; saved when it changed
    /// (`on_loaded` clears it on every refresh of a clean repository).
    pub fn set_pending(&mut self, pending: Option<Pending>, git_dir: &Path) {
        if self.pending != pending {
            self.pending = pending;
            self.save(git_dir);
        }
    }

    fn file(git_dir: &Path) -> PathBuf {
        git_dir.join("gf").join("undo.json")
    }

    /// The saved history; empty when there is none or it cannot be read.
    pub fn load(git_dir: &Path) -> Self {
        std::fs::read(Self::file(git_dir))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Saves the history in the background: it can hold thousands of
    /// paths, too much to write on the UI thread after every operation.
    /// Saves finish in the order they were asked for (an older one never
    /// overwrites a newer one).
    pub fn save(&self, git_dir: &Path) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        static WRITTEN: std::sync::Mutex<Option<HashMap<PathBuf, u64>>> =
            std::sync::Mutex::new(None);

        let seq = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = Self::file(git_dir);
        let history = self.clone();
        std::thread::spawn(move || {
            let mut written = WRITTEN
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let last = written.get_or_insert_with(HashMap::new);
            if last.get(&path).is_some_and(|&l| l > seq) {
                return;
            }
            if let Err(e) = history.write(&path) {
                tracing::warn!(error = %e, "saving undo history");
            }
            last.insert(path, seq);
        });
    }

    fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, path)
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `3m ago` style age of a step.
pub fn age(at: u64) -> String {
    let secs = now().saturating_sub(at);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

// ------------------------------------------------------------ scopes

/// Paths with any change (both sides of a rename, untracked folders).
pub fn changed_paths(status: &StatusReport) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in &status.entries {
        if let StatusEntry::Ignored { .. } = e {
            continue;
        }
        out.push(e.path().clone());
        if let StatusEntry::Changed {
            orig_path: Some(o), ..
        } = e
        {
            out.push(o.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Staging or unstaging `paths`.
pub fn index(paths: Vec<PathBuf>) -> Scope {
    Scope {
        index: paths,
        ..Scope::default()
    }
}

/// Changing the work tree files `paths` (discard, partial discard).
pub fn files(paths: Vec<PathBuf>) -> Scope {
    Scope {
        files: paths,
        ..Scope::default()
    }
}

/// Resolving a conflict: the file and its index entries (all stages).
pub fn resolve(path: PathBuf) -> Scope {
    file_and_index(path)
}

/// A file gf rewrites that may also get staged (`.gitattributes` for
/// `lfs track`): its content and its index entry.
pub fn file_and_index(path: PathBuf) -> Scope {
    Scope {
        index: vec![path.clone()],
        files: vec![path],
        ..Scope::default()
    }
}

/// Which commit is checked out in each of `paths` (submodule update).
pub fn submodules(paths: Vec<PathBuf>) -> Scope {
    Scope {
        submodules: paths,
        ..Scope::default()
    }
}

/// Creating, deleting or renaming branches (with their upstream config).
pub fn branches(names: &[&str], head: bool) -> Scope {
    Scope {
        head,
        refs: names.iter().map(|n| format!("refs/heads/{n}")).collect(),
        config: names.iter().map(|n| format!("branch.{n}")).collect(),
        ..Scope::default()
    }
}

pub fn tag(name: &str) -> Scope {
    Scope {
        refs: vec![format!("refs/tags/{name}")],
        ..Scope::default()
    }
}

/// Stash operations: the list, and the changed files they take or give.
pub fn stash(paths: Vec<PathBuf>) -> Scope {
    Scope {
        stash: true,
        index: paths.clone(),
        files: paths,
        ..Scope::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(label: &str) -> Step {
        Step::new(
            label.into(),
            Scope::default(),
            Capture::default(),
            Capture::default(),
            HeadMode::Keep,
        )
    }

    #[test]
    fn undo_and_redo_move_steps_between_stacks() {
        let mut h = History::default();
        h.push(step("a"));
        h.push(step("b"));
        h.applied(false);
        assert_eq!(h.next(false).map(|s| s.label.as_str()), Some("a"));
        assert_eq!(h.next(true).map(|s| s.label.as_str()), Some("b"));
        h.applied(true);
        assert_eq!(h.next(false).map(|s| s.label.as_str()), Some("b"));
        h.applied(false);
        // A new step drops what could be redone.
        h.push(step("c"));
        assert!(h.next(true).is_none());
    }

    #[test]
    fn only_a_branch_that_stays_checked_out_is_checked_for_pushed_commits() {
        let head = |branch: &str, oid: Option<&str>| Capture {
            head: Some(gf_git::Head {
                branch: Some(branch.into()),
                oid: oid.map(Into::into),
            }),
            ..Capture::default()
        };
        let commit = Step::new(
            "commit".into(),
            Scope::head(),
            head("main", Some("a")),
            head("main", Some("b")),
            HeadMode::Soft,
        );
        assert_eq!(commit.branch_move(false), Some(("main", "b", Some("a"))));
        assert_eq!(commit.branch_move(true), Some(("main", "a", Some("b"))));
        let first = Step::new(
            "first commit".into(),
            Scope::head(),
            head("main", None),
            head("main", Some("a")),
            HeadMode::Soft,
        );
        assert_eq!(first.branch_move(false), Some(("main", "a", None)));
        let checkout = Step::new(
            "checkout".into(),
            Scope::head(),
            head("main", Some("a")),
            head("dev", Some("c")),
            HeadMode::Keep,
        );
        assert_eq!(checkout.branch_move(false), None);
        assert_eq!(step("stage").branch_move(false), None);
    }

    #[test]
    fn saved_copies_of_both_stacks_are_kept_once() {
        let blob = |oid: &str| Capture {
            files: vec![(
                "a.txt".into(),
                Some(gf_git::FileContent::Blob {
                    oid: oid.into(),
                    executable: false,
                }),
            )],
            ..Capture::default()
        };
        let mut h = History::default();
        let mut discard = step("discard");
        discard.before = blob("b1");
        h.push(discard);
        let mut redo = step("ignore");
        redo.before = blob("b1");
        redo.after = blob("b2");
        h.push(redo);
        h.applied(false);
        h.push(step("stage"));
        // `push` dropped what could be redone: only its copies go.
        assert_eq!(h.saved_blobs(), ["b1"]);
        h.applied(false);
        h.applied(false);
        assert_eq!(h.saved_blobs(), ["b1"], "undone steps keep theirs");
    }

    #[test]
    fn a_batch_moves_the_branch_from_its_first_head_step_to_its_last() {
        let head = |oid: &str| Capture {
            head: Some(gf_git::Head {
                branch: Some("main".into()),
                oid: Some(oid.into()),
            }),
            ..Capture::default()
        };
        let commit = |label: &str, from: &str, to: &str| {
            Step::new(
                label.into(),
                Scope::head(),
                head(from),
                head(to),
                HeadMode::Soft,
            )
        };
        let mut h = History::default();
        h.push(commit("c1", "a", "b"));
        h.push(step("stage"));
        h.push(commit("c2", "b", "c"));
        let steps = h.upcoming(false, 3);
        let labels: Vec<&str> = steps.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["c2", "stage", "c1"], "run order");
        assert_eq!(batch_move(&steps, false), Some(("main", "c", Some("a"))));
        assert_eq!(batch_move(&steps[1..2], false), None, "no HEAD step");
        let mut q = Queue::new(false, 3);
        assert_eq!((q.left, q.done()), (2, 1));
        q.left -= 1;
        assert_eq!(q.done(), 2);
    }

    #[test]
    fn history_is_capped() {
        let mut h = History::default();
        for i in 0..MAX_STEPS + 3 {
            h.push(step(&i.to_string()));
        }
        assert_eq!(h.undo.len(), MAX_STEPS);
        assert_eq!(h.undo[0].label, "3");
    }

    #[test]
    fn saved_history_loads_back_with_redo_and_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = History::default();
        h.push(step("a"));
        h.push(step("b"));
        h.applied(false);
        h.pending = Some(Pending {
            label: "merge x".into(),
            scope: Scope::head(),
            before: Capture::default(),
            mode: HeadMode::Keep,
        });
        h.write(&History::file(dir.path())).expect("write");
        let loaded = History::load(dir.path());
        assert_eq!(loaded.undo, h.undo);
        assert_eq!(loaded.redo, h.redo);
        assert_eq!(loaded.pending, h.pending);
        assert!(History::load(&dir.path().join("none")).undo.is_empty());
    }

    #[test]
    fn histories_saved_before_redo_was_kept_still_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = History::file(dir.path());
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file, br#"{"undo":[]}"#).expect("write");
        let loaded = History::load(dir.path());
        assert!(loaded.redo.is_empty() && loaded.pending.is_none());
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age(now()), "just now");
        assert_eq!(age(now() - 180), "3m ago");
        assert_eq!(age(now() - 7200), "2h ago");
    }
}
