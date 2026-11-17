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
}

/// An operation that stopped on conflicts: its step is recorded once it
/// is continued to the end (and dropped when aborted).
#[derive(Debug, Clone)]
pub struct Pending {
    pub label: String,
    pub scope: Scope,
    pub before: Capture,
    pub mode: HeadMode,
}

/// Several undo / redo steps asked at once (from the history list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Queue {
    pub redo: bool,
    /// Steps still to run after the current one.
    pub left: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    /// Oldest first; the last one is undone next.
    pub undo: Vec<Step>,
    /// Undone steps, the last one is redone next. Not saved.
    #[serde(skip)]
    pub redo: Vec<Step>,
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

    fn file(git_dir: &Path) -> PathBuf {
        git_dir.join("gf").join("undo.json")
    }

    /// The saved undo stack; empty when there is none or it cannot be read.
    pub fn load(git_dir: &Path) -> Self {
        std::fs::read(Self::file(git_dir))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Saves the undo stack in the background: it can hold thousands of
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
        let undo = self.undo.clone();
        std::thread::spawn(move || {
            let mut written = WRITTEN
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let last = written.get_or_insert_with(HashMap::new);
            if last.get(&path).is_some_and(|&l| l > seq) {
                return;
            }
            let history = Self {
                undo,
                redo: Vec::new(),
            };
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
    Scope {
        index: vec![path.clone()],
        files: vec![path],
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
    fn history_is_capped() {
        let mut h = History::default();
        for i in 0..MAX_STEPS + 3 {
            h.push(step(&i.to_string()));
        }
        assert_eq!(h.undo.len(), MAX_STEPS);
        assert_eq!(h.undo[0].label, "3");
    }

    #[test]
    fn saved_undo_stack_loads_back_without_redo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = History::default();
        h.push(step("a"));
        h.push(step("b"));
        h.applied(false);
        h.write(&History::file(dir.path())).expect("write");
        let loaded = History::load(dir.path());
        assert_eq!(loaded.undo, h.undo);
        assert!(loaded.redo.is_empty());
        assert!(History::load(&dir.path().join("none")).undo.is_empty());
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age(now()), "just now");
        assert_eq!(age(now() - 180), "3m ago");
        assert_eq!(age(now() - 7200), "2h ago");
    }
}
