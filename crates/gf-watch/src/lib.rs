//! Watches a repository on disk and publishes what changed to the
//! [`EventBus`]: working tree files, the index, refs (branches, tags, HEAD)
//! or config. Events are debounced so a burst of writes (a build, a checkout)
//! produces one refresh.

use std::collections::HashSet;
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
/// Changed paths checked against `.gitignore` per batch; past this the
/// batch is reported as "many files" unchecked.
const MAX_CHECKED: usize = 20_000;

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
            | "refs" | "reftable" | "shallow" => Some(Change::Refs),
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

/// Paths `git check-ignore` already reported as ignored, so a build that
/// rewrites the same outputs every few hundred milliseconds does not start
/// a git process for each batch. Forgotten whenever what decides it may
/// have changed: a `.gitignore`, `info/exclude` or the index (a file added
/// with `git add -f` is no longer ignored).
#[derive(Debug, Default)]
pub struct IgnoreCache {
    ignored: HashSet<PathBuf>,
}

impl IgnoreCache {
    /// Entries kept at most; past this the cache starts over.
    const MAX: usize = 50_000;

    /// `paths` without ignored ones, asking git only about paths not
    /// known to be ignored.
    pub fn drop_ignored(&mut self, workdir: &Path, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let unknown: Vec<PathBuf> = paths
            .iter()
            .filter(|p| !self.ignored.contains(*p))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            let kept = drop_ignored(workdir, unknown.clone());
            let kept: HashSet<&PathBuf> = kept.iter().collect();
            if self.ignored.len() + unknown.len() > Self::MAX {
                self.ignored.clear();
            }
            self.ignored
                .extend(unknown.iter().filter(|p| !kept.contains(p)).cloned());
        }
        paths
            .into_iter()
            .filter(|p| !self.ignored.contains(p))
            .collect()
    }

    /// Forgets everything (the ignore rules or the index changed).
    pub fn clear(&mut self) {
        self.ignored.clear();
    }

    /// Whether a change to `path` (relative to the work tree) can change
    /// what is ignored.
    fn is_rule_file(path: &Path) -> bool {
        path.file_name().is_some_and(|n| n == ".gitignore")
    }
}

/// `paths` without the ones `.gitignore` (and `info/exclude`, the global
/// excludes) ignores, as `git check-ignore` decides. Tracked files are
/// never dropped. On any error the paths are kept as they are.
pub fn drop_ignored(workdir: &Path, paths: Vec<PathBuf>) -> Vec<PathBuf> {
    match ignored(workdir, &paths) {
        Ok(ignored) if !ignored.is_empty() => {
            paths.into_iter().filter(|p| !ignored.contains(p)).collect()
        }
        Ok(_) => paths,
        Err(e) => {
            tracing::debug!(error = %e, "check-ignore failed; keeping every changed path");
            paths
        }
    }
}

fn ignored(workdir: &Path, paths: &[PathBuf]) -> std::io::Result<HashSet<PathBuf>> {
    use std::io::{Read, Write};
    use std::process::Stdio;
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let mut cmd = gf_core::tool_path::git_command();
    cmd.arg("-C")
        .arg(workdir)
        .args(["check-ignore", "--stdin", "-z"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(path_bytes(p).as_ref());
        input.push(0);
    }
    let mut stdin = child.stdin.take().ok_or(std::io::ErrorKind::BrokenPipe)?;
    // Written from another thread: a long answer would otherwise fill the
    // output pipe while the input is still being written.
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let mut out = Vec::new();
    child
        .stdout
        .take()
        .ok_or(std::io::ErrorKind::BrokenPipe)?
        .read_to_end(&mut out)?;
    let status = child.wait()?;
    let _ = writer.join();
    // 0: some ignored, 1: none; anything else is an error (e.g. a path
    // inside a submodule).
    if !matches!(status.code(), Some(0 | 1)) {
        return Err(std::io::Error::other(format!("git check-ignore: {status}")));
    }
    Ok(out
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(path_from_bytes)
        .collect())
}

#[cfg(unix)]
fn path_bytes(p: &Path) -> std::borrow::Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    std::borrow::Cow::Borrowed(p.as_os_str().as_bytes())
}

#[cfg(not(unix))]
fn path_bytes(p: &Path) -> std::borrow::Cow<'_, [u8]> {
    std::borrow::Cow::Owned(p.to_string_lossy().into_owned().into_bytes())
}

#[cfg(unix)]
fn path_from_bytes(b: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(b))
}

#[cfg(not(unix))]
fn path_from_bytes(b: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(b).into_owned())
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
        let mut ignore_cache = IgnoreCache::default();
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
            for e in &events {
                match classify(&e.path, handler_workdir.as_deref(), &handler_git_dir) {
                    Some(Change::WorkTree(p)) => {
                        if paths.len() < MAX_CHECKED {
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
            let exclude = handler_git_dir.join("info").join("exclude");
            if index
                || events
                    .iter()
                    .any(|e| IgnoreCache::is_rule_file(&e.path) || e.path == exclude)
            {
                ignore_cache.clear();
            }
            // Build output, `node_modules`, logs…: ignored files change
            // nothing gf shows, so a build does not refresh it over and over.
            if !many {
                if let Some(w) = handler_workdir.as_deref() {
                    paths = ignore_cache.drop_ignored(w, paths);
                }
                many = paths.len() > MAX_PATHS;
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
    fn ignored_paths_are_dropped_and_tracked_ones_kept() {
        let repo = gf_test_support::TestRepo::new();
        repo.write(".gitignore", "target/\n*.log\n");
        repo.write("kept.log", "tracked although it matches\n");
        repo.git(&["add", "-f", "kept.log", ".gitignore"]);
        repo.git(&["commit", "-qm", "init"]);
        repo.write("target/debug/x", "");
        repo.write("build.log", "");
        repo.write("src/a.rs", "");
        let p = |s: &str| PathBuf::from(s);
        let left = drop_ignored(
            repo.path(),
            vec![
                p("target"),
                p("target/debug/x"),
                p("build.log"),
                p("kept.log"),
                p("src/a.rs"),
            ],
        );
        assert_eq!(left, [p("kept.log"), p("src/a.rs")]);
    }

    #[test]
    fn ignored_paths_are_remembered_until_cleared() {
        let repo = gf_test_support::TestRepo::new();
        repo.write(".gitignore", "*.log\n");
        repo.git(&["add", ".gitignore"]);
        repo.git(&["commit", "-qm", "init"]);
        let p = |s: &str| PathBuf::from(s);
        let mut cache = IgnoreCache::default();
        let left = cache.drop_ignored(repo.path(), vec![p("a.log"), p("a.rs")]);
        assert_eq!(left, [p("a.rs")]);

        // Without git: the rules no longer ignore `*.log`, the cache still does.
        repo.write(".gitignore", "");
        let left = cache.drop_ignored(repo.path(), vec![p("a.log")]);
        assert!(left.is_empty(), "answered from the cache");
        cache.clear();
        let left = cache.drop_ignored(repo.path(), vec![p("a.log")]);
        assert_eq!(left, [p("a.log")], "asked git again");
        assert!(IgnoreCache::is_rule_file(Path::new("sub/.gitignore")));
    }

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
        for path in ["/r/.git/shallow", "/r/.git/reftable/tables.list"] {
            assert_eq!(classify(Path::new(path), Some(w), g), Some(Change::Refs));
        }
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
