//! One open repository: its git handle, the last loaded data snapshot, the
//! file watcher, and per-repository feature state (selection, collapsed
//! sections, commit draft...), so switching tabs preserves everything.

use std::sync::Arc;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gf_core::model::{Branch, Commit, Oid, Remote, StashEntry, StatusEntry, StatusReport, Tag};
use gf_git::graph::{self, GraphRow};
use gf_git::{
    BisectProgress, LineCounts, LogOrder, LogQuery, RepoState, Repository, Submodule, Worktree,
};
use gf_watch::RepoWatcher;

use crate::features::{changes, commit_box, diff_view, history, sidebar, worktrees};

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
        revision: Option<String>,
        limit: usize,
    ) -> gf_core::Result<Self> {
        let query = LogQuery {
            revisions: revision.into_iter().collect(),
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

/// Which work tree files changed since the last refresh, as far as the
/// file watcher knows: decides whether an open work tree diff is read
/// again. Anything but a watcher event (an operation, ⌘R, the index or
/// refs moving) may have changed any file: [`Touched::all`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Touched {
    all: bool,
    paths: std::collections::BTreeSet<PathBuf>,
}

impl Touched {
    /// More paths than this are not kept: the refresh treats every file as
    /// changed.
    const MAX_PATHS: usize = 256;

    /// Any file may have changed.
    pub fn all() -> Self {
        Self {
            all: true,
            paths: std::collections::BTreeSet::new(),
        }
    }

    /// These files (or folders) changed. An empty list means the watcher
    /// saw too many to list.
    pub fn paths(paths: &[PathBuf]) -> Self {
        if paths.is_empty() || paths.len() > Self::MAX_PATHS {
            return Self::all();
        }
        Self {
            all: false,
            paths: paths.iter().cloned().collect(),
        }
    }

    /// Whether `path` (relative to the work tree) may have changed.
    pub fn includes(&self, path: &Path) -> bool {
        self.all || self.paths.iter().any(|p| path.starts_with(p))
    }
}

impl std::ops::BitOrAssign for Touched {
    fn bitor_assign(&mut self, o: Self) {
        if self.all || o.all || self.paths.len() + o.paths.len() > Self::MAX_PATHS {
            *self = Self::all();
        } else {
            self.paths.extend(o.paths);
        }
    }
}

/// Local tags no remote has: the user's own, not yet published. A tag
/// fetched from any remote (an `upstream` the fork tracks, say) does not
/// count, so push never sends another project's tags to `remote`. Each
/// remote's tag list is what `gf_git` recorded at the last listing, fetch or
/// push (see `remote_tags.rs`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingTags {
    /// The remote pushes go to: the upstream's, else `origin`, else the first.
    pub remote: String,
    pub names: Vec<String>,
}

/// Local tag names that none of the `known` remote tag lists contains.
pub fn tags_on_no_remote<'a>(
    local: impl IntoIterator<Item = &'a str>,
    known: &[std::collections::BTreeSet<String>],
) -> Vec<String> {
    local
        .into_iter()
        .filter(|n| !known.iter().any(|k| k.contains(*n)))
        .map(str::to_owned)
        .collect()
}

/// The remote a push of HEAD goes to: its upstream's remote, else
/// `origin`, else the first one. `None` without remotes.
pub fn push_remote(upstream: Option<&str>, remotes: &[Remote]) -> Option<String> {
    upstream
        .and_then(|up| {
            // Remote names may contain `/`: match the configured ones.
            remotes
                .iter()
                .filter(|r| up.starts_with(&format!("{}/", r.name)))
                .max_by_key(|r| r.name.len())
        })
        .or_else(|| remotes.iter().find(|r| r.name == "origin"))
        .or_else(|| remotes.first())
        .map(|r| r.name.clone())
}

/// Everything the views show for a repository. Each refresh reads the
/// [`Parts`] asked for and keeps the others.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub status: StatusReport,
    /// Indices into status entries and their directory layout.
    pub change_files: Arc<crate::features::changes::FileLists>,
    /// Whether the loaded status has changes other than ignored files.
    pub has_changes: bool,
    pub branches: Arc<[Branch]>,
    pub tags: Arc<[Tag]>,
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
    /// Commits reachable from the current branch that no remote-tracking
    /// branch contains.
    pub unpushed_commits: Arc<HashSet<Oid>>,
    /// Local tags the push remote does not have (see [`PendingTags`]);
    /// `None` when gf does not know that remote's tags yet.
    pub pending_tags: Option<PendingTags>,
    /// Remotes whose tags gf has never listed: until they are, which
    /// tags are unpublished is unknown (`pending_tags` is `None`).
    pub unlisted_remotes: Vec<String>,
    /// When the repository was last fetched (`FETCH_HEAD`'s timestamp).
    pub last_fetch: Option<std::time::SystemTime>,
    /// Merge / rebase / cherry-pick / revert / bisect in progress.
    pub state: RepoState,
    pub worktrees: Vec<Worktree>,
    pub submodules: Vec<Submodule>,
    /// LFS-tracked patterns; `None` when the repository does not use LFS.
    pub lfs: Option<Vec<String>>,
    /// Branch and tag badges per commit, built with the refs they show.
    pub badges: Arc<crate::badges::Badges>,
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
    /// Size and modification time of each counted untracked file, so the
    /// next refresh only reads the files that changed.
    untracked_stamps: HashMap<PathBuf, Stamp>,
}

/// What identifies a file's content cheaply: its size and modification
/// time. A file whose stamp did not change keeps its line count.
type Stamp = (u64, Option<std::time::SystemTime>);

/// Untracked files larger than this are not read to count lines.
/// Newest unpushed commits marked in History. Beyond it (a branch with no
/// remote and a huge history) the marker is left off rather than holding
/// every id.
const UNPUSHED_MARK_LIMIT: usize = 5000;

const MAX_COUNT_BYTES: u64 = 2 * 1024 * 1024;
/// At most this many untracked files are counted per refresh.
const MAX_COUNT_FILES: usize = 2000;

/// Lines in a new file (all of them are additions). `None` for binary or
/// unreadable/huge files.
#[cfg(test)]
fn count_new_file(path: &Path) -> Option<LineCounts> {
    count_bytes(std::fs::metadata(path).ok()?.len(), path)
}

/// [`count_new_file`] for a file of `len` bytes.
fn count_bytes(len: u64, path: &Path) -> Option<LineCounts> {
    if len > MAX_COUNT_BYTES {
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

        let mut head_changed = false;
        if let Some(status) = status {
            let status = status.map_err(err)?;
            head_changed = status.head != snap.status.head;
            snap.change_files = Arc::new(crate::features::changes::FileLists::new(&status.entries));
            snap.status = status;
            snap.has_changes = snap
                .status
                .entries
                .iter()
                .any(|e| !matches!(e, StatusEntry::Ignored { .. }));
            // Counts are decoration: a failure must not block the refresh.
            snap.lines.staged = staged.and_then(Result::ok).unwrap_or_default();
            snap.lines.unstaged = unstaged.and_then(Result::ok).unwrap_or_default();
            let (counts, stamps) = count_untracked(&repo, &snap.status, &snap.lines).await;
            snap.lines.untracked = counts;
            snap.lines.untracked_stamps = stamps;
        }
        if let Some(b) = branches {
            snap.branches = b.map_err(err)?.into();
        }
        if let Some(t) = tags {
            snap.tags = t.map_err(err)?.into();
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
            let head = &snap.status.head;
            if head.upstream.is_some() || head.branch.is_none() {
                snap.unpublished = None;
            } else if refs_part
                || head_changed
                || snap.unpublished.is_none()
                || repo.handle().git_dir != repo.cwd().join(".git")
            {
                // File edits do not change reachability. Linked worktrees
                // watch their private git dir, so a shared remote ref can
                // move without a refs event in this tab.
                snap.unpublished = repo.unpushed_count().await.ok();
            }
        }
        if refs_part || head_changed || repo.handle().git_dir != repo.cwd().join(".git") {
            if snap.status.head.branch.is_some() && snap.status.head.oid.is_some() {
                snap.unpushed_commits = match repo.unpushed_recent(UNPUSHED_MARK_LIMIT).await {
                    Ok(commits) => Arc::new(commits.into_iter().collect()),
                    Err(e) => {
                        tracing::warn!(error = %e, "could not load unpushed commits");
                        Arc::default()
                    }
                };
            } else {
                snap.unpushed_commits = Arc::default();
            }
        }

        if refs_part {
            // Tags, remotes and the upstream only change with the refs or
            // the config: a saved file does not read the list again.
            let mut known = Vec::new();
            snap.unlisted_remotes.clear();
            for r in &snap.remotes {
                match repo.known_remote_tags(&r.name).await {
                    Some(k) => known.push(k),
                    None => snap.unlisted_remotes.push(r.name.clone()),
                }
            }
            snap.pending_tags = push_remote(snap.status.head.upstream.as_deref(), &snap.remotes)
                .filter(|_| snap.unlisted_remotes.is_empty())
                .map(|remote| PendingTags {
                    remote,
                    names: tags_on_no_remote(snap.tags.iter().map(|t| t.name.short()), &known),
                });
        }

        if refs_part || head_changed {
            snap.badges = Arc::new(crate::badges::build(
                &snap.branches,
                &snap.tags,
                &snap.status.head,
            ));
        }

        let key = if refs_part || parts.history {
            history_key(
                snap.status.head.oid.as_ref(),
                &snap.branches,
                &snap.tags,
                &snap.stashes,
                all_branches,
                history_limit,
            )
        } else {
            snap.history_key
        };
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

    /// Switching branch has nothing to ask about: HEAD is on a branch (no
    /// commits could be left behind), no merge, rebase or bisect is in
    /// progress, and nothing is changed (ignored files aside). Even if the
    /// status is a moment old, `git switch` refuses to overwrite a change.
    pub fn can_switch_quietly(&self) -> bool {
        self.status.head.branch.is_some()
            && self.state.is_clean()
            && self
                .status
                .entries
                .iter()
                .all(|e| matches!(e, StatusEntry::Ignored { .. }))
    }

    /// The newest stash gf made when changes were left on the current
    /// branch ("stash, then switch"), to offer them back.
    pub fn left_here(&self) -> Option<&StashEntry> {
        let branch = self.status.head.branch.as_deref()?;
        self.stashes
            .iter()
            .find(|s| gf_git::left_on_branch(&s.message) == Some(branch))
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
/// UI thread. Files whose size and modification time match `prev` keep
/// their count without being read again (a saved file reads one file, not
/// every untracked one).
async fn count_untracked(
    repo: &Repository,
    status: &StatusReport,
    prev: &LineStats,
) -> (Counts, HashMap<PathBuf, Stamp>) {
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
    let prev_counts = prev.untracked.clone();
    let prev_stamps = prev.untracked_stamps.clone();
    tokio::task::spawn_blocking(move || count_files(&root, new_files, &prev_counts, &prev_stamps))
        .await
        .unwrap_or_default()
}

/// Counts `files` (relative to `root`), reusing `prev` for unchanged ones.
fn count_files(
    root: &Path,
    files: Vec<PathBuf>,
    prev: &Counts,
    prev_stamps: &HashMap<PathBuf, Stamp>,
) -> (Counts, HashMap<PathBuf, Stamp>) {
    let mut counts = Counts::with_capacity(files.len());
    let mut stamps = HashMap::with_capacity(files.len());
    for p in files {
        let full = root.join(&p);
        let Ok(meta) = std::fs::metadata(&full) else {
            counts.insert(p, None);
            continue;
        };
        let stamp = (meta.len(), meta.modified().ok());
        let count = match (prev_stamps.get(&p), prev.get(&p)) {
            (Some(s), Some(c)) if *s == stamp => *c,
            _ => count_bytes(stamp.0, &full),
        };
        counts.insert(p.clone(), count);
        stamps.insert(p, stamp);
    }
    (counts, stamps)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageUpdate {
    Running,
    Refreshing,
}

#[derive(Debug)]
pub struct RepoTab {
    pub repo: Repository,
    pub snapshot: Option<Arc<Snapshot>>,
    pub loading: bool,
    /// The user asked for a refresh (⌘R): say so when it is done, as a
    /// refresh that finds nothing new changes nothing on screen.
    pub refresh_asked: bool,
    /// Parts asked for and not read yet: while a refresh runs, or while
    /// the tab is in the background (see [`RepoTab::load_due`]).
    pub queued: Parts,
    /// Files changed for the queued parts.
    pub touched: Touched,
    /// Files changed for the refresh that is running.
    pub loading_touched: Touched,
    pub error: Option<String>,
    /// Show every branch in the history graph instead of HEAD only.
    pub all_branches: bool,
    /// Held only to keep the file watcher running for this tab.
    _watcher: Option<RepoWatcher>,
    pub sidebar: sidebar::State,
    /// The refs panel's other mode (⌘⇧B): every working tree as a card.
    pub worktrees: worktrees::State,
    pub changes: changes::State,
    pub history: history::State,
    /// A rewrite plan belongs to the repository that produced it.
    pub rebase_editor: Option<crate::features::rebase_editor::State>,
    /// Changes of a working tree file (editable).
    pub local_diff: diff_view::State,
    /// Changes of a commit or stash (read-only, with a file list).
    pub commit_diff: diff_view::State,
    /// Main area mode: `Commit` = HISTORY over CHANGES (default),
    /// `Local` = WORKING TREE over CHANGES. The CHANGES view shows the
    /// matching diff (commit changes / local changes).
    pub code_view: diff_view::Role,
    pub commit: commit_box::State,
    pub stage_update: Option<StageUpdate>,
    /// Blame shown instead of the commit changes while open.
    pub blame_request: u64,
    pub history_request: u64,
    pub blame: Option<crate::features::blame_view::State>,
    /// Conflict resolver shown instead of the local diff while open.
    pub conflict: Option<crate::features::conflict_view::State>,
    /// Undo / redo steps of gf's own operations (saved in the git dir).
    pub undo: crate::undo::History,
    /// An undo / redo is being checked or run: further presses wait.
    pub undo_busy: bool,
    /// More undo / redo steps to run after the current one.
    pub undo_queue: Option<crate::undo::Queue>,
    /// Latest bisect step result.
    pub bisect: Option<BisectProgress>,
    /// Stashes of left changes whose "bring back" offer was dismissed.
    pub left_dismissed: Vec<Oid>,
    /// The commit-graph was checked for this tab (see `App::on_loaded`).
    pub graph_checked: bool,
    /// Remotes whose tags this session listed (see `App::on_loaded`):
    /// each once, so a remote added later is listed too.
    pub remote_tags_listed: std::collections::HashSet<String>,
}

impl RepoTab {
    pub fn new(repo: Repository, watcher: Option<RepoWatcher>) -> Self {
        let undo = crate::undo::History::load(&repo.handle().git_dir);
        Self {
            repo,
            snapshot: None,
            loading: false,
            refresh_asked: false,
            // Read by `App::load_due` once the tab's turn comes.
            queued: Parts::RELOAD,
            touched: Touched::all(),
            loading_touched: Touched::default(),
            error: None,
            all_branches: true,
            _watcher: watcher,
            sidebar: sidebar::State::default(),
            worktrees: worktrees::State::default(),
            changes: changes::State::default(),
            history: history::State::default(),
            rebase_editor: None,
            local_diff: diff_view::State::new(diff_view::Role::Local),
            commit_diff: diff_view::State::new(diff_view::Role::Commit),
            code_view: diff_view::Role::Commit,
            blame_request: 0,
            history_request: 0,
            blame: None,
            conflict: None,
            undo,
            undo_busy: false,
            undo_queue: None,
            bisect: None,
            left_dismissed: Vec::new(),
            graph_checked: false,
            remote_tags_listed: std::collections::HashSet::new(),
            commit: commit_box::State::default(),
            stage_update: None,
        }
    }

    /// Whether the queued parts should be read now. Only one refresh runs
    /// per tab. The shown tab reads everything at once; a background tab
    /// reads ref and config changes right away (its tab label shows the
    /// branch) but leaves file and index changes, the frequent ones (every
    /// save, every build), until it is shown again.
    pub fn load_due(&self, active: bool) -> bool {
        !self.loading
            && !self.queued.is_empty()
            && (active || self.queued.refs || self.queued.config)
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
    fn switching_quietly_needs_a_branch_no_operation_and_no_changes() {
        let clean = Snapshot {
            status: StatusReport {
                head: gf_core::model::HeadInfo {
                    branch: Some("main".into()),
                    ..Default::default()
                },
                entries: vec![StatusEntry::Ignored {
                    path: "target".into(),
                }],
            },
            ..Snapshot::default()
        };
        assert!(clean.can_switch_quietly(), "ignored files don't count");

        let mut s = clean.clone();
        s.status.head.branch = None;
        assert!(!s.can_switch_quietly(), "detached HEAD");

        let mut s = clean.clone();
        s.state = gf_git::RepoState::Bisecting;
        assert!(!s.can_switch_quietly(), "operation in progress");

        let mut s = clean;
        s.status.entries.push(StatusEntry::Untracked {
            path: "notes.txt".into(),
        });
        assert!(!s.can_switch_quietly(), "a change");
    }

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

    #[test]
    fn touched_files_decide_what_counts_as_changed() {
        let p = |s: &str| PathBuf::from(s);
        let saved = Touched::paths(&[p("src/a.rs"), p("new")]);
        assert!(saved.includes(Path::new("src/a.rs")));
        assert!(
            saved.includes(Path::new("new/inner.txt")),
            "inside a folder"
        );
        assert!(!saved.includes(Path::new("src/b.rs")));
        assert!(
            Touched::paths(&[]).includes(Path::new("x")),
            "too many to list"
        );
        assert!(Touched::all().includes(Path::new("x")));
        assert!(!Touched::default().includes(Path::new("x")), "nothing");

        let mut both = saved.clone();
        both |= Touched::paths(&[p("src/b.rs")]);
        assert!(both.includes(Path::new("src/a.rs")) && both.includes(Path::new("src/b.rs")));
        both |= Touched::all();
        assert_eq!(both, Touched::all());
        let many: Vec<PathBuf> = (0..=Touched::MAX_PATHS)
            .map(|i| p(&i.to_string()))
            .collect();
        assert_eq!(Touched::paths(&many), Touched::all(), "capped");
    }

    #[tokio::test]
    async fn background_tabs_wait_for_file_changes_but_not_for_refs() {
        let repo = gf_test_support::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.commit_all("init");
        let runner = gf_git::GitRunner::new()
            .with_cleared_env()
            .with_env(repo.env());
        let r = Repository::open(repo.path(), runner).await.expect("open");
        let mut tab = RepoTab::new(r, None);
        assert!(
            tab.load_due(false),
            "a new tab loads, also in the background"
        );
        tab.queued = Parts::default();
        tab.snapshot = Some(Arc::default());

        tab.queued = Parts::STATUS;
        assert!(tab.load_due(true), "shown: at once");
        assert!(!tab.load_due(false), "background: a saved file waits");
        tab.queued |= Parts {
            refs: true,
            ..Parts::default()
        };
        assert!(tab.load_due(false), "background: a moved branch does not");
        tab.loading = true;
        assert!(!tab.load_due(true), "one load at a time");
    }

    #[test]
    fn unchanged_untracked_files_are_not_read_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "1\n2\n").expect("write");
        std::fs::write(dir.path().join("b.txt"), "1\n").expect("write");
        let files = || vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];
        let (counts, stamps) = count_files(dir.path(), files(), &Counts::new(), &HashMap::new());
        assert_eq!(counts[Path::new("a.txt")].map(|c| c.added), Some(2));

        // A count only a reuse could produce proves the file was not read.
        let mut planted = counts.clone();
        planted.insert(
            "a.txt".into(),
            Some(LineCounts {
                added: 99,
                removed: 0,
            }),
        );
        std::fs::write(dir.path().join("b.txt"), "1\n2\n3\n").expect("write");
        let (again, _) = count_files(dir.path(), files(), &planted, &stamps);
        assert_eq!(
            again[Path::new("a.txt")].map(|c| c.added),
            Some(99),
            "reused"
        );
        assert_eq!(
            again[Path::new("b.txt")].map(|c| c.added),
            Some(3),
            "re-read"
        );

        // A file gone between the status and the count has no count.
        let (gone, _) = count_files(dir.path(), vec!["c.txt".into()], &planted, &stamps);
        assert_eq!(gone[Path::new("c.txt")], None);
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
        assert!(
            Arc::ptr_eq(&saved.unpushed_commits, &first.unpushed_commits),
            "unpushed commits reused"
        );
        assert!(!saved.loaded.history);
        assert!(Arc::ptr_eq(&saved.badges, &first.badges));
        assert!(Arc::ptr_eq(&saved.branches, &first.branches));
        assert!(Arc::ptr_eq(&saved.tags, &first.tags));
        assert_eq!(saved.unpublished, Some(1));

        // A refresh after an operation reads the refs; they did not move.
        let same = load(Some(saved.clone()), Parts::ALL).await.expect("all");
        assert!(Arc::ptr_eq(&same.history, &first.history));

        repo.commit_all("second");
        let edited_head = load(Some(same), Parts::STATUS).await.expect("HEAD moved");
        assert_eq!(edited_head.unpublished, Some(2));
        let moved = load(Some(edited_head), Parts::ALL)
            .await
            .expect("refs moved");
        assert!(moved.loaded.history);
        assert_eq!(moved.history.commits.len(), 2);

        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let fetched = load(Some(moved), Parts::ALL)
            .await
            .expect("remote refs moved");
        assert_eq!(fetched.unpublished, Some(0));
    }

    #[tokio::test]
    async fn unpushed_commits_refresh_when_refs_or_head_change() {
        let repo = gf_test_support::TestRepo::new();
        repo.write("base.txt", "base\n");
        let base = repo.commit_all("base");
        let current_branch = repo.git(&["branch", "--show-current"]);
        repo.git(&["branch", "side"]);
        repo.git(&["switch", "side"]);
        repo.write("side.txt", "side\n");
        let side = repo.commit_all("side");
        repo.git(&["switch", current_branch.as_str()]);
        repo.write("main.txt", "main\n");
        let current = repo.commit_all("current");

        let runner = gf_git::GitRunner::new()
            .with_cleared_env()
            .with_env(repo.env());
        let r = Repository::open(repo.path(), runner).await.expect("open");
        let load = |prev, parts| Snapshot::load(r.clone(), prev, parts, false, 100);
        let oid = |id: &str| id.parse::<Oid>().expect("oid");

        let first = load(None, Parts::RELOAD).await.expect("first load");
        assert_eq!(first.unpushed_commits.len(), 2);
        assert!(first.unpushed_commits.contains(&oid(&base)));
        assert!(first.unpushed_commits.contains(&oid(&current)));
        assert!(
            !first.unpushed_commits.contains(&oid(&side)),
            "commits only on another local branch are not included"
        );

        repo.git(&["update-ref", "refs/remotes/origin/main", base.as_str()]);
        repo.git(&[
            "update-ref",
            "refs/remotes/upstream/shared",
            current.as_str(),
        ]);
        let all_remote_refs = load(Some(first), Parts::ALL)
            .await
            .expect("remote refs added");
        assert!(
            all_remote_refs.unpushed_commits.is_empty(),
            "commits on any remote-tracking branch are excluded"
        );

        repo.git(&["update-ref", "refs/remotes/upstream/shared", base.as_str()]);
        let moved_remote_ref = load(Some(all_remote_refs), Parts::ALL)
            .await
            .expect("remote ref moved");
        assert_eq!(moved_remote_ref.status.head.oid, Some(oid(&current)));
        assert_eq!(moved_remote_ref.unpushed_commits.len(), 1);
        assert!(moved_remote_ref.unpushed_commits.contains(&oid(&current)));

        repo.git(&["switch", "side"]);
        let switched_branch = load(Some(moved_remote_ref), Parts::STATUS)
            .await
            .expect("branch switched");
        assert_eq!(switched_branch.unpushed_commits.len(), 1);
        assert!(switched_branch.unpushed_commits.contains(&oid(&side)));
        assert!(!switched_branch.unpushed_commits.contains(&oid(&current)));

        repo.git(&["switch", "--detach", base.as_str()]);
        let detached = load(Some(switched_branch), Parts::STATUS)
            .await
            .expect("detached HEAD");
        assert!(detached.status.head.branch.is_none());
        assert!(detached.unpushed_commits.is_empty());

        repo.git(&["switch", "--orphan", "empty"]);
        let unborn = load(Some(detached), Parts::STATUS)
            .await
            .expect("unborn branch");
        assert!(unborn.status.head.oid.is_none());
        assert!(unborn.unpushed_commits.is_empty());
    }

    #[tokio::test]
    async fn linked_worktree_sees_shared_remote_refs_on_a_status_refresh() {
        let main = gf_test_support::TestRepo::new();
        main.write("a", "first");
        main.commit_all("init");
        let parent = tempfile::tempdir().expect("temp dir");
        let path = parent.path().join("linked");
        main.git(&[
            "worktree",
            "add",
            "-b",
            "linked",
            path.to_str().expect("path"),
        ]);
        let runner = gf_git::GitRunner::new()
            .with_cleared_env()
            .with_env(main.env());
        let repo = Repository::open(path, runner).await.expect("linked repo");
        let first = Snapshot::load(repo.clone(), None, Parts::RELOAD, false, 100)
            .await
            .expect("initial load");
        assert_eq!(first.unpublished, Some(1));
        assert_eq!(first.unpushed_commits.len(), 1);
        main.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let next = Snapshot::load(repo, Some(first), Parts::STATUS, false, 100)
            .await
            .expect("status");
        assert_eq!(next.unpublished, Some(0));
        assert!(next.unpushed_commits.is_empty());
    }

    #[tokio::test]
    async fn a_new_tag_shows_after_the_refresh_an_operation_asks_for() {
        let repo = gf_test_support::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.commit_all("init");
        let runner = gf_git::GitRunner::new()
            .with_cleared_env()
            .with_env(repo.env());
        let r = Repository::open(repo.path(), runner).await.expect("open");
        let load = |prev, parts| Snapshot::load(r.clone(), prev, parts, false, 100);
        let first = load(None, Parts::RELOAD).await.expect("first load");
        assert!(first.tags.is_empty());

        r.create_tag("v1", None, None).await.expect("lightweight");
        let after = load(Some(first), Parts::ALL).await.expect("refresh");
        let names: Vec<_> = after.tags.iter().map(|t| t.name.short()).collect();
        assert_eq!(names, ["v1"]);

        r.create_tag("v2", None, Some("notes"))
            .await
            .expect("annotated");
        let after = load(Some(after), Parts::ALL).await.expect("refresh");
        let names: Vec<_> = after.tags.iter().map(|t| t.name.short()).collect();
        assert_eq!(names, ["v1", "v2"]);
        assert!(after.badges.values().flatten().any(|b| b.label == "◆ v2"));
    }

    #[test]
    fn push_goes_to_the_upstream_remote_else_origin() {
        let r = |n: &str| Remote {
            name: n.into(),
            fetch_refspecs: Vec::new(),
            fetch_url: String::new(),
            push_url: None,
        };
        let remotes = [r("fork"), r("origin"), r("team/a")];
        assert_eq!(
            push_remote(Some("fork/main"), &remotes).as_deref(),
            Some("fork")
        );
        assert_eq!(
            push_remote(Some("team/a/x"), &remotes).as_deref(),
            Some("team/a")
        );
        assert_eq!(push_remote(None, &remotes).as_deref(), Some("origin"));
        assert_eq!(push_remote(None, &remotes[..1]).as_deref(), Some("fork"));
        assert_eq!(push_remote(None, &[]), None);
    }

    #[test]
    fn only_tags_no_remote_has_are_unpublished() {
        let set = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
        // A fork: `origin` has few tags, `upstream` the project's releases.
        let known = [set(&["mine-1"]), set(&["release-1", "release-2"])];
        let local = ["release-1", "release-2", "mine-1", "mine-2"];
        assert_eq!(tags_on_no_remote(local, &known), ["mine-2"]);
        assert_eq!(tags_on_no_remote(local, &[]).len(), 4);
    }

    #[tokio::test]
    async fn only_tags_on_no_remote_count_and_pushing_clears_them() {
        use gf_test_support::TestRepo;
        let origin = TestRepo::new_bare();
        let upstream = TestRepo::new_bare();
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.commit_all("init");
        repo.git(&[
            "remote",
            "add",
            "origin",
            origin.path().to_str().expect("utf-8 path"),
        ]);
        repo.git(&[
            "remote",
            "add",
            "upstream",
            upstream.path().to_str().expect("utf-8 path"),
        ]);
        repo.git(&["push", "-q", "-u", "origin", "main"]);
        // The project's release lives on `upstream` only; `mine` nowhere.
        repo.git(&["tag", "release-1"]);
        repo.git(&["push", "-q", "upstream", "release-1"]);
        repo.git(&["tag", "mine"]);
        let runner = gf_git::GitRunner::new()
            .with_cleared_env()
            .with_env(repo.env());
        let r = Repository::open(repo.path(), runner).await.expect("open");
        let load = |prev, parts| Snapshot::load(r.clone(), prev, parts, false, 100);

        let first = load(None, Parts::RELOAD).await.expect("load");
        assert_eq!(first.pending_tags, None, "unknown before any listing");
        assert_eq!(first.unlisted_remotes, ["origin", "upstream"]);

        r.record_remote_tags("origin").await.expect("origin");
        let half = load(Some(first), Parts::ALL).await.expect("load");
        assert_eq!(half.pending_tags, None, "still unknown: upstream unlisted");
        assert_eq!(half.unlisted_remotes, ["upstream"]);

        r.record_remote_tags("upstream").await.expect("upstream");
        let listed = load(Some(half), Parts::ALL).await.expect("load");
        let pending = listed.pending_tags.clone().expect("known");
        assert_eq!(pending.remote, "origin", "the upstream branch's remote");
        assert_eq!(pending.names, ["mine"], "upstream's release is not ours");

        // A tag on another branch counts the same.
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.write("b.txt", "b\n");
        repo.commit_all("side");
        repo.git(&["tag", "on-side"]);
        repo.git(&["checkout", "-q", "main"]);
        let more = load(Some(listed), Parts::ALL).await.expect("load");
        assert_eq!(
            more.pending_tags.clone().expect("known").names,
            ["mine", "on-side"]
        );

        // Pushed through gf: recorded, no longer counted.
        r.push_tags("origin", &["mine".into(), "on-side".into()])
            .await
            .expect("push");
        let pushed = load(Some(more), Parts::ALL).await.expect("load");
        assert!(
            pushed
                .pending_tags
                .as_ref()
                .expect("known")
                .names
                .is_empty()
        );

        // Pushed from a terminal: counted until the list is read again
        // (fetch, or the next time the repository is opened).
        repo.git(&["tag", "cli"]);
        repo.git(&["push", "-q", "origin", "cli"]);
        let stale = load(Some(pushed), Parts::ALL).await.expect("load");
        assert_eq!(stale.pending_tags.clone().expect("known").names, ["cli"]);
        r.record_remote_tags("origin").await.expect("relist");
        let fresh = load(Some(stale), Parts::ALL).await.expect("load");
        assert!(fresh.pending_tags.as_ref().expect("known").names.is_empty());
    }
}
