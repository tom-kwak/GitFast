//! Watches a repository on disk and publishes what changed to the
//! [`EventBus`]: working tree files, the index, refs (branches, tags, HEAD)
//! or config. Events are debounced so a burst of writes (a build, a checkout)
//! produces one refresh.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use gf_core::model::RepoId;
use gf_core::{CoreEvent, EventBus, GfError, Result};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

const DEBOUNCE: Duration = Duration::from_millis(300);
/// More changed paths than this in one batch are reported as a single
/// "many files" event (an empty path list).
const MAX_PATHS: usize = 200;

/// What a changed path means for the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    WorkTree(PathBuf),
    Index,
    Refs,
    Config,
}

/// Classifies one changed path. `None` for files that never affect what gf
/// shows (objects, logs, lock files).
pub fn classify(path: &Path, workdir: Option<&Path>, git_dir: &Path) -> Option<Change> {
    if let Ok(rel) = path.strip_prefix(git_dir) {
        let first = rel.components().next()?;
        let Component::Normal(first) = first else {
            return None;
        };
        return match first.to_str()? {
            "index" => Some(Change::Index),
            // Refs, plus the markers of an operation in progress (they
            // decide the banner: merge, cherry-pick, revert, rebase, bisect).
            "HEAD" | "ORIG_HEAD" | "FETCH_HEAD" | "MERGE_HEAD" | "CHERRY_PICK_HEAD"
            | "REVERT_HEAD" | "BISECT_LOG" | "rebase-merge" | "rebase-apply" | "packed-refs"
            | "refs" => Some(Change::Refs),
            "config" => Some(Change::Config),
            _ => None,
        };
    }
    let rel = path.strip_prefix(workdir?).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    Some(Change::WorkTree(rel.to_path_buf()))
}

/// Keeps watching while alive; dropping it stops the watcher.
pub struct RepoWatcher {
    _debouncer: Debouncer<RecommendedWatcher>,
}

impl std::fmt::Debug for RepoWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoWatcher").finish_non_exhaustive()
    }
}

impl RepoWatcher {
    /// Starts watching. Paths should be canonical (as `git rev-parse`
    /// reports them) so they match what the OS reports.
    pub fn start(
        repo: RepoId,
        workdir: Option<PathBuf>,
        git_dir: PathBuf,
        bus: EventBus,
    ) -> Result<Self> {
        let handler_git_dir = git_dir.clone();
        let handler_workdir = workdir.clone();
        let mut debouncer = new_debouncer(DEBOUNCE, move |result: DebounceEventResult| {
            let events = match result {
                Ok(events) => events,
                Err(e) => {
                    tracing::warn!(error = %e, "file watcher error");
                    return;
                }
            };
            let (mut paths, mut index, mut refs, mut config, mut many) =
                (Vec::new(), false, false, false, false);
            for e in events {
                match classify(&e.path, handler_workdir.as_deref(), &handler_git_dir) {
                    Some(Change::WorkTree(p)) => {
                        if paths.len() < MAX_PATHS {
                            paths.push(p);
                        } else {
                            many = true;
                        }
                    }
                    Some(Change::Index) => index = true,
                    Some(Change::Refs) => refs = true,
                    Some(Change::Config) => config = true,
                    None => {}
                }
            }
            if many {
                paths.clear();
                bus.publish(CoreEvent::WorkTreeChanged { repo, paths });
            } else if !paths.is_empty() {
                bus.publish(CoreEvent::WorkTreeChanged { repo, paths });
            }
            if index {
                bus.publish(CoreEvent::IndexChanged(repo));
            }
            if refs {
                bus.publish(CoreEvent::RefsChanged(repo));
            }
            if config {
                bus.publish(CoreEvent::ConfigChanged(repo));
            }
        })
        .map_err(|e| GfError::Other(format!("starting file watcher: {e}")))?;

        let watch = |d: &mut Debouncer<RecommendedWatcher>, p: &Path| {
            d.watcher()
                .watch(p, RecursiveMode::Recursive)
                .map_err(|e| GfError::Other(format!("watching {}: {e}", p.display())))
        };
        match &workdir {
            Some(w) => {
                watch(&mut debouncer, w)?;
                // A git dir outside the work tree (worktrees, --separate-git-dir).
                if !git_dir.starts_with(w) {
                    watch(&mut debouncer, &git_dir)?;
                }
            }
            None => watch(&mut debouncer, &git_dir)?,
        }
        Ok(Self {
            _debouncer: debouncer,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn classifies_paths() {
        let w = Path::new("/r");
        let g = Path::new("/r/.git");
        assert_eq!(
            classify(Path::new("/r/src/a.rs"), Some(w), g),
            Some(Change::WorkTree("src/a.rs".into()))
        );
        assert_eq!(
            classify(Path::new("/r/.git/index"), Some(w), g),
            Some(Change::Index)
        );
        assert_eq!(
            classify(Path::new("/r/.git/refs/heads/main"), Some(w), g),
            Some(Change::Refs)
        );
        assert_eq!(
            classify(Path::new("/r/.git/HEAD"), Some(w), g),
            Some(Change::Refs)
        );
        assert_eq!(
            classify(Path::new("/r/.git/config"), Some(w), g),
            Some(Change::Config)
        );
        assert_eq!(
            classify(Path::new("/r/.git/objects/ab/cd"), Some(w), g),
            None
        );
        assert_eq!(classify(Path::new("/r/.git/index.lock"), Some(w), g), None);
        assert_eq!(
            classify(Path::new("/r/.git/rebase-merge/done"), Some(w), g),
            Some(Change::Refs)
        );
        assert_eq!(
            classify(Path::new("/r/.git/CHERRY_PICK_HEAD"), Some(w), g),
            Some(Change::Refs)
        );
        assert_eq!(classify(Path::new("/elsewhere/x"), Some(w), g), None);
        assert_eq!(classify(Path::new("/r"), Some(w), g), None);
    }

    #[tokio::test]
    async fn reports_worktree_index_and_ref_changes() {
        use gf_test_support::TestRepo;

        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.commit_all("init");
        let root = repo.path().canonicalize().unwrap();
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let id = RepoId::new();
        let _w = RepoWatcher::start(id, Some(root.clone()), root.join(".git"), bus).unwrap();
        // FSEvents needs a moment before it reports changes.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut wait_for = async |pred: &dyn Fn(&CoreEvent) -> bool| {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let e = rx.recv().await.unwrap();
                    if pred(&e) {
                        return;
                    }
                }
            })
            .await
            .is_ok()
        };

        repo.write("b.txt", "b\n");
        assert!(
            wait_for(&|e| matches!(e, CoreEvent::WorkTreeChanged { paths, .. } if paths.iter().any(|p| p == Path::new("b.txt"))))
                .await,
            "work tree change"
        );
        repo.git(&["add", "b.txt"]);
        assert!(
            wait_for(&|e| matches!(e, CoreEvent::IndexChanged(r) if *r == id)).await,
            "index change"
        );
        repo.git(&["branch", "topic"]);
        assert!(
            wait_for(&|e| matches!(e, CoreEvent::RefsChanged(r) if *r == id)).await,
            "refs change"
        );
    }
}
