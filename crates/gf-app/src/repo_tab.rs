//! One open repository: its git handle, the last loaded data snapshot, the
//! file watcher, and per-repository feature state (selection, collapsed
//! sections, commit draft...), so switching tabs preserves everything.

use std::sync::Arc;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gf_core::model::{Branch, Commit, Oid, Remote, StashEntry, StatusEntry, StatusReport, Tag};
use gf_git::graph::{self, GraphRow};
use gf_git::{
    BisectProgress, LineCounts, LogOrder, LogQuery, RepoState, Repository, Submodule, Worktree,
};
use gf_watch::RepoWatcher;

use crate::features::{changes, commit_box, diff_view, history, sidebar};

/// Commits per history page: the list starts with this many and loads
/// another page each time it is scrolled to the end.
pub const HISTORY_PAGE: usize = 1000;
/// Number of lane colors the graph cycles through (see `history`).
pub const LANE_COLORS: usize = 6;

/// Commits plus their graph rows, computed off the UI thread.
#[derive(Debug, Clone, Default)]
pub struct History {
    pub commits: Vec<Commit>,
    pub graph: Vec<GraphRow>,
    /// More commits exist past `commits` (the load hit its limit).
    pub more: bool,
}

impl History {
    pub fn new(commits: Vec<Commit>) -> Self {
        let graph = graph::layout(&commits, LANE_COLORS);
        Self {
            commits,
            graph,
            more: false,
        }
    }

    /// The first `limit` commits of `commits`; `more` when there were
    /// more than that.
    fn limited(mut commits: Vec<Commit>, limit: usize) -> Self {
        let more = commits.len() > limit;
        commits.truncate(limit);
        Self {
            more,
            ..Self::new(commits)
        }
    }

    /// Up to `limit` commits that touched `path`, following renames.
    pub async fn load_path(
        repo: &Repository,
        path: PathBuf,
        limit: usize,
    ) -> gf_core::Result<Self> {
        let query = LogQuery {
            paths: vec![path],
            follow: true,
            // One extra tells whether there is more.
            max_count: Some(limit + 1),
            ..LogQuery::default()
        };
        Ok(Self::limited(repo.log(&query).await?, limit))
    }

    /// Up to `limit` commits of `revisions` (HEAD when empty) or of all
    /// refs. Topological order makes a longer load extend a shorter one,
    /// so the rows already on screen stay put.
    pub async fn load(
        repo: &Repository,
        revisions: Vec<String>,
        all_refs: bool,
        limit: usize,
    ) -> gf_core::Result<Self> {
        let query = LogQuery {
            revisions,
            all_refs,
            max_count: Some(limit + 1),
            order: LogOrder::Topo,
            ..LogQuery::default()
        };
        Ok(Self::limited(repo.log(&query).await?, limit))
    }
}

/// Which parts of a [`Snapshot`] a refresh reads again; the rest is kept
/// from the previous snapshot. The file watcher says what changed, so a
/// saved file reads the status only, not the history (thousands of
/// commits and their graph) or the submodules (a `git` process each).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Parts {
    /// Status, line counts.
    pub status: bool,
    /// Branches, tags, stashes, worktrees, operation in progress (and the
    /// status: ahead / behind and HEAD come with it).
    pub refs: bool,
    /// Remotes and upstreams.
    pub config: bool,
    /// Submodules and LFS patterns.
    pub modules: bool,
    /// The history even when the refs it shows did not move (⌘R).
    pub history: bool,
}

impl Parts {
    /// Everything; the history only if its refs moved.
    pub const ALL: Self = Self {
        status: true,
        refs: true,
        config: true,
        modules: true,
        history: false,
    };
    /// Everything, the history included (⌘R, first load).
    pub const RELOAD: Self = Self {
        history: true,
        ..Self::ALL
    };
    pub const STATUS: Self = Self {
        status: true,
        refs: false,
        config: false,
        modules: false,
        history: false,
    };
    /// One more page of history.
    pub const HISTORY: Self = Self {
        status: false,
        refs: false,
        config: false,
        modules: false,
        history: true,
    };

    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

impl std::ops::BitOr for Parts {
    type Output = Self;
    fn bitor(self, o: Self) -> Self {
        Self {
            status: self.status || o.status,
            refs: self.refs || o.refs,
            config: self.config || o.config,
            modules: self.modules || o.modules,
            history: self.history || o.history,
        }
    }
}

impl std::ops::BitOrAssign for Parts {
    fn bitor_assign(&mut self, o: Self) {
        *self = *self | o;
    }
}

/// Everything the views show for a repository. Each refresh reads the
/// [`Parts`] asked for and keeps the others.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub status: StatusReport,
    pub branches: Vec<Branch>,
    pub tags: Vec<Tag>,
    pub remotes: Vec<Remote>,
    pub stashes: Vec<StashEntry>,
    /// Shared between snapshots while its refs do not move.
    pub history: Arc<History>,
    /// What the history was loaded for (see [`history_key`]).
    pub(crate) history_key: u64,
    /// `+added −removed` per file for the working tree list.
    pub lines: LineStats,
    /// For a branch without upstream: commits no remote has yet (what a
    /// first push would publish). `None` when the branch has an upstream.
    pub unpublished: Option<u32>,
    /// When the repository was last fetched (`FETCH_HEAD`'s timestamp).
    pub last_fetch: Option<std::time::SystemTime>,
    /// Merge / rebase / cherry-pick / revert / bisect in progress.
    pub state: RepoState,
    pub worktrees: Vec<Worktree>,
    pub submodules: Vec<Submodule>,
    /// LFS-tracked patterns; `None` when the repository does not use LFS.
    pub lfs: Option<Vec<String>>,
    /// What this snapshot's refresh read (the rest came from the last one).
    pub loaded: Parts,
}

/// Line counts per path; `None` = binary file.
pub type Counts = HashMap<PathBuf, Option<LineCounts>>;

#[derive(Debug, Clone, Default)]
pub struct LineStats {
    pub staged: Counts,
    pub unstaged: Counts,
    /// Untracked files count as all-added.
    pub untracked: Counts,
}

/// Untracked files larger than this are not read to count lines.
const MAX_COUNT_BYTES: u64 = 2 * 1024 * 1024;
/// At most this many untracked files are counted per refresh.
const MAX_COUNT_FILES: usize = 2000;

/// Lines in a new file (all of them are additions). `None` for binary or
/// unreadable/huge files.
fn count_new_file(path: &Path) -> Option<LineCounts> {
    if std::fs::metadata(path).ok()?.len() > MAX_COUNT_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    let partial = usize::from(bytes.last().is_some_and(|&b| b != b'\n'));
    Some(LineCounts {
        added: u32::try_from(newlines + partial).unwrap_or(u32::MAX),
        removed: 0,
    })
}

/// Everything the history list depends on: when it is the same as last
/// time, the commits and their graph are reused instead of read again.
/// `git log --all` also walks `refs/stash`, hence the stashes.
fn history_key(
    head: Option<&Oid>,
    branches: &[Branch],
    tags: &[Tag],
    stashes: &[StashEntry],
    all_branches: bool,
    limit: usize,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (head, all_branches, limit).hash(&mut h);
    for b in branches {
        (b.name.as_str(), &b.target).hash(&mut h);
    }
    for t in tags {
        (t.name.as_str(), &t.target).hash(&mut h);
    }
    for s in stashes {
        s.id.hash(&mut h);
    }
    h.finish()
}

impl Snapshot {
    /// Reads `parts` of the repository (all of them without `prev`),
    /// concurrently, and keeps the rest from `prev`.
    /// `history_limit`: how many commits of history to load.
    pub async fn load(
        repo: Repository,
        prev: Option<Arc<Self>>,
        parts: Parts,
        all_branches: bool,
        history_limit: usize,
    ) -> Result<Arc<Self>, String> {
        let started = std::time::Instant::now();
        let (parts, mut snap) = match prev {
            Some(p) => (parts, (*p).clone()),
            None => (Parts::RELOAD, Self::default()),
        };
        // HEAD and ahead / behind are part of the status.
        let status_part = parts.status || parts.refs || parts.config;
        let refs_part = parts.refs || parts.config;
        let err = |e: gf_core::GfError| e.to_string();

        let lfs = async {
            if !parts.modules {
                return None;
            }
            let uses_lfs = tokio::fs::read_to_string(repo.cwd().join(".gitattributes"))
                .await
                .is_ok_and(|a| a.contains("filter=lfs"));
            Some(if uses_lfs && repo.lfs_available().await {
                repo.lfs_tracked().await.ok()
            } else {
                None
            })
        };
        let (status, staged, unstaged, branches, tags, stashes, remotes) = tokio::join!(
            async {
                if status_part {
                    Some(repo.status().await)
                } else {
                    None
                }
            },
            async {
                if status_part {
                    Some(repo.numstat(true).await)
                } else {
                    None
                }
            },
            async {
                if status_part {
                    Some(repo.numstat(false).await)
                } else {
                    None
                }
            },
            async {
                if refs_part {
                    Some(repo.branches().await)
                } else {
                    None
                }
            },
            async {
                if parts.refs {
                    Some(repo.tags().await)
                } else {
                    None
                }
            },
            async {
                if parts.refs {
                    Some(repo.stashes().await)
                } else {
                    None
                }
            },
            async {
                if parts.config {
                    Some(repo.remotes().await)
                } else {
                    None
                }
            },
        );
        let (worktrees, state, submodules, lfs) = tokio::join!(
            async {
                if parts.refs {
                    Some(repo.worktrees().await)
                } else {
                    None
                }
            },
            async {
                if parts.refs {
                    Some(repo.state().await)
                } else {
                    None
                }
            },
            async {
                if parts.modules {
                    Some(repo.submodules().await)
                } else {
                    None
                }
            },
            lfs,
        );

        if let Some(status) = status {
            snap.status = status.map_err(err)?;
            // Counts are decoration: a failure must not block the refresh.
            snap.lines.staged = staged.and_then(Result::ok).unwrap_or_default();
            snap.lines.unstaged = unstaged.and_then(Result::ok).unwrap_or_default();
            snap.lines.untracked = count_untracked(&repo, &snap.status).await;
        }
        if let Some(b) = branches {
            snap.branches = b.map_err(err)?;
        }
        if let Some(t) = tags {
            snap.tags = t.map_err(err)?;
        }
        if let Some(s) = stashes {
            snap.stashes = s.map_err(err)?;
        }
        if let Some(r) = remotes {
            snap.remotes = r.map_err(err)?;
        }
        // Optional sections: a failure only hides them.
        if let Some(w) = worktrees {
            snap.worktrees = w.unwrap_or_default();
        }
        if let Some(s) = state {
            snap.state = s.unwrap_or_default();
        }
        if let Some(s) = submodules {
            snap.submodules = s.unwrap_or_default();
        }
        if let Some(l) = lfs {
            snap.lfs = l;
        }
        if parts.refs {
            snap.last_fetch = tokio::fs::metadata(repo.handle().git_dir.join("FETCH_HEAD"))
                .await
                .and_then(|m| m.modified())
                .ok();
        }
        if status_part {
            // Without an upstream, ahead/behind are unknown; count what a
            // first push would publish instead.
            snap.unpublished =
                if snap.status.head.upstream.is_none() && snap.status.head.branch.is_some() {
                    repo.unpushed_count().await.ok()
                } else {
                    None
                };
        }

        let key = history_key(
            snap.status.head.oid.as_ref(),
            &snap.branches,
            &snap.tags,
            &snap.stashes,
            all_branches,
            history_limit,
        );
        // Only a refresh that read the refs knows where they are now (a
        // status-only one sees a new HEAD before the branch it moved).
        let history = parts.history || (refs_part && key != snap.history_key);
        if history {
            snap.history = Arc::new(
                History::load(&repo, Vec::new(), all_branches, history_limit)
                    .await
                    .map_err(err)?,
            );
            snap.history_key = key;
        }
        snap.loaded = Parts {
            status: status_part,
            refs: refs_part,
            history,
            ..parts
        };
        tracing::debug!(
            parts = ?snap.loaded,
            elapsed = ?started.elapsed(),
            "snapshot loaded"
        );
        Ok(Arc::new(snap))
    }

    /// Branch HEAD points to, if any.
    pub fn head_branch(&self) -> Option<&Branch> {
        self.branches.iter().find(|b| b.is_head)
    }

    pub fn staged_count(&self) -> usize {
        self.status.entries.iter().filter(|e| e.is_staged()).count()
    }

    pub fn conflict_count(&self) -> usize {
        self.status
            .entries
            .iter()
            .filter(|e| matches!(e, StatusEntry::Conflicted { .. }))
            .count()
    }
}

/// Line counts of new files (they have no diff to count), read off the
/// UI thread.
async fn count_untracked(repo: &Repository, status: &StatusReport) -> Counts {
    let root = repo.cwd().to_path_buf();
    let new_files: Vec<PathBuf> = status
        .entries
        .iter()
        .filter_map(|e| match e {
            StatusEntry::Untracked { path } => Some(path.clone()),
            _ => None,
        })
        .take(MAX_COUNT_FILES)
        .collect();
    tokio::task::spawn_blocking(move || {
        new_files
            .into_iter()
            .map(|p| {
                let counts = count_new_file(&root.join(&p));
                (p, counts)
            })
            .collect::<Counts>()
    })
    .await
    .unwrap_or_default()
}

#[derive(Debug)]
pub struct RepoTab {
    pub repo: Repository,
    pub snapshot: Option<Arc<Snapshot>>,
    pub loading: bool,
    /// Parts asked for while a refresh was running; read once it is done.
    pub queued: Parts,
    pub error: Option<String>,
    /// Show every branch in the history graph instead of HEAD only.
    pub all_branches: bool,
    /// Held only to keep the file watcher running for this tab.
    _watcher: Option<RepoWatcher>,
    pub sidebar: sidebar::State,
    pub changes: changes::State,
    pub history: history::State,
    /// Changes of a working tree file (editable).
    pub local_diff: diff_view::State,
    /// Changes of a commit or stash (read-only, with a file list).
    pub commit_diff: diff_view::State,
    /// Main area mode: `Commit` = HISTORY over CHANGES (default),
    /// `Local` = WORKING TREE over CHANGES. The CHANGES view shows the
    /// matching diff (commit changes / local changes).
    pub code_view: diff_view::Role,
    pub commit: commit_box::State,
    /// Blame shown instead of the commit changes while open.
    pub blame: Option<crate::features::blame_view::State>,
    /// Conflict resolver shown instead of the local diff while open.
    pub conflict: Option<crate::features::conflict_view::State>,
    /// Undo / redo steps of gf's own operations (saved in the git dir).
    pub undo: crate::undo::History,
    /// An operation stopped on conflicts, recorded once it is continued.
    pub pending: Option<crate::undo::Pending>,
    /// An undo / redo is being checked or run: further presses wait.
    pub undo_busy: bool,
    /// More undo / redo steps to run after the current one.
    pub undo_queue: Option<crate::undo::Queue>,
    /// Latest bisect step result.
    pub bisect: Option<BisectProgress>,
}

impl RepoTab {
    pub fn new(repo: Repository, watcher: Option<RepoWatcher>) -> Self {
        let undo = crate::undo::History::load(&repo.handle().git_dir);
        Self {
            repo,
            snapshot: None,
            loading: true,
            queued: Parts::default(),
            error: None,
            all_branches: true,
            _watcher: watcher,
            sidebar: sidebar::State::default(),
            changes: changes::State::default(),
            history: history::State::default(),
            local_diff: diff_view::State::new(diff_view::Role::Local),
            commit_diff: diff_view::State::new(diff_view::Role::Commit),
            code_view: diff_view::Role::Commit,
            blame: None,
            conflict: None,
            undo,
            pending: None,
            undo_busy: false,
            undo_queue: None,
            bisect: None,
            commit: commit_box::State::default(),
        }
    }

    pub fn name(&self) -> String {
        self.repo.handle().display_name()
    }

    /// Current branch name, or `HEAD@<id>` when detached.
    pub fn head_label(&self) -> Option<String> {
        let head = &self.snapshot.as_ref()?.status.head;
        Some(match (&head.branch, &head.oid) {
            (Some(b), _) => b.clone(),
            (None, Some(oid)) => format!("HEAD@{}", oid.short()),
            (None, None) => "(no commits)".into(),
        })
    }
}

impl RepoTab {
    pub fn diff(&self, role: diff_view::Role) -> &diff_view::State {
        match role {
            diff_view::Role::Local => &self.local_diff,
            diff_view::Role::Commit => &self.commit_diff,
        }
    }

    pub fn diff_mut(&mut self, role: diff_view::Role) -> &mut diff_view::State {
        match role {
            diff_view::Role::Local => &mut self.local_diff,
            diff_view::Role::Commit => &mut self.commit_diff,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lines_of_new_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let text = dir.path().join("a.txt");
        std::fs::write(&text, "one\ntwo\nthree").expect("write");
        assert_eq!(
            count_new_file(&text),
            Some(LineCounts {
                added: 3,
                removed: 0
            })
        );
        let empty = dir.path().join("e.txt");
        std::fs::write(&empty, "").expect("write");
        assert_eq!(count_new_file(&empty), Some(LineCounts::default()));
        let bin = dir.path().join("b.bin");
        std::fs::write(&bin, [0u8, 1, 2]).expect("write");
        assert_eq!(count_new_file(&bin), None);
    }

    #[tokio::test]
    async fn a_saved_file_reloads_the_status_but_reuses_the_history() {
        let repo = gf_test_support::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.commit_all("init");
        let runner = gf_git::GitRunner::new()
            .with_cleared_env()
            .with_env(repo.env());
        let r = Repository::open(repo.path(), runner).await.expect("open");
        let load = |prev, parts| Snapshot::load(r.clone(), prev, parts, false, 100);

        let first = load(None, Parts::STATUS).await.expect("first load");
        assert_eq!(first.loaded, Parts::RELOAD, "a first load reads everything");
        assert_eq!(first.history.commits.len(), 1);

        repo.write("a.txt", "edited\n");
        let saved = load(Some(first.clone()), Parts::STATUS)
            .await
            .expect("status");
        assert_eq!(saved.status.entries.len(), 1, "the edit shows");
        assert!(
            Arc::ptr_eq(&saved.history, &first.history),
            "history reused"
        );
        assert!(!saved.loaded.history);

        // A refresh after an operation reads the refs; they did not move.
        let same = load(Some(saved.clone()), Parts::ALL).await.expect("all");
        assert!(Arc::ptr_eq(&same.history, &first.history));

        repo.commit_all("second");
        let moved = load(Some(same), Parts::ALL).await.expect("refs moved");
        assert!(moved.loaded.history);
        assert_eq!(moved.history.commits.len(), 2);
    }
}
