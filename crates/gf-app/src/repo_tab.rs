//! One open repository: its git handle, the last loaded data snapshot, the
//! file watcher, and per-repository feature state (selection, collapsed
//! sections, commit draft...), so switching tabs preserves everything.

use std::sync::Arc;

use gf_core::model::{Branch, Commit, Remote, StashEntry, StatusReport, Tag};
use gf_git::graph::{self, GraphRow};
use gf_git::{LogOrder, LogQuery, Repository};
use gf_watch::RepoWatcher;

use crate::features::{changes, commit_box, diff_view, history, sidebar};

/// How many commits the history loads.
pub const HISTORY_LIMIT: usize = 1000;
/// Number of lane colors the graph cycles through (see `history`).
pub const LANE_COLORS: usize = 6;

/// Commits plus their graph rows, computed off the UI thread.
#[derive(Debug, Clone, Default)]
pub struct History {
    pub commits: Vec<Commit>,
    pub graph: Vec<GraphRow>,
}

impl History {
    pub fn new(commits: Vec<Commit>) -> Self {
        let graph = graph::layout(&commits, LANE_COLORS);
        Self { commits, graph }
    }

    /// Loads the history of `revisions` (HEAD when empty) or of all refs.
    pub async fn load(
        repo: &Repository,
        revisions: Vec<String>,
        all_refs: bool,
    ) -> gf_core::Result<Self> {
        let query = LogQuery {
            revisions,
            all_refs,
            max_count: Some(HISTORY_LIMIT),
            order: LogOrder::Topo,
            ..LogQuery::default()
        };
        Ok(Self::new(repo.log(&query).await?))
    }
}

/// Everything the views show for a repository, loaded together.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub status: StatusReport,
    pub branches: Vec<Branch>,
    pub tags: Vec<Tag>,
    pub remotes: Vec<Remote>,
    pub stashes: Vec<StashEntry>,
    pub history: History,
}

impl Snapshot {
    /// Loads all parts concurrently.
    pub async fn load(repo: Repository, all_branches: bool) -> Result<Arc<Self>, String> {
        let (status, branches, tags, remotes, stashes, history) = tokio::join!(
            repo.status(),
            repo.branches(),
            repo.tags(),
            repo.remotes(),
            repo.stashes(),
            History::load(&repo, Vec::new(), all_branches),
        );
        let err = |e: gf_core::GfError| e.to_string();
        Ok(Arc::new(Self {
            status: status.map_err(err)?,
            branches: branches.map_err(err)?,
            tags: tags.map_err(err)?,
            remotes: remotes.map_err(err)?,
            stashes: stashes.map_err(err)?,
            history: history.map_err(err)?,
        }))
    }

    /// Branch HEAD points to, if any.
    pub fn head_branch(&self) -> Option<&Branch> {
        self.branches.iter().find(|b| b.is_head)
    }

    pub fn staged_count(&self) -> usize {
        self.status.entries.iter().filter(|e| e.is_staged()).count()
    }
}

#[derive(Debug)]
pub struct RepoTab {
    pub repo: Repository,
    pub snapshot: Option<Arc<Snapshot>>,
    pub loading: bool,
    /// A refresh was requested while loading; run another one afterwards.
    pub reload_pending: bool,
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
}

impl RepoTab {
    pub fn new(repo: Repository, watcher: Option<RepoWatcher>) -> Self {
        Self {
            repo,
            snapshot: None,
            loading: true,
            reload_pending: false,
            error: None,
            all_branches: true,
            _watcher: watcher,
            sidebar: sidebar::State::default(),
            changes: changes::State::default(),
            history: history::State::default(),
            local_diff: diff_view::State::new(diff_view::Role::Local),
            commit_diff: diff_view::State::new(diff_view::Role::Commit),
            code_view: diff_view::Role::Commit,
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
