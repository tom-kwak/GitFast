//! One open repository: its git handle, the last loaded data snapshot and
//! the per-repository state of features (selection, collapsed sections...),
//! so switching tabs preserves everything.

use std::sync::Arc;

use gf_core::model::{Branch, Commit, Remote, StatusReport, Tag};
use gf_git::{LogQuery, Repository};

use crate::features::{changes, history, sidebar};

/// How many commits the overview loads. The full graph arrives in M3.
const HISTORY_LIMIT: usize = 500;

/// Everything the views show for a repository, loaded together.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub status: StatusReport,
    pub branches: Vec<Branch>,
    pub tags: Vec<Tag>,
    pub remotes: Vec<Remote>,
    pub commits: Vec<Commit>,
}

impl Snapshot {
    /// Loads all parts concurrently.
    pub async fn load(repo: Repository) -> Result<Arc<Self>, String> {
        let query = LogQuery::head(HISTORY_LIMIT);
        let (status, branches, tags, remotes, commits) = tokio::join!(
            repo.status(),
            repo.branches(),
            repo.tags(),
            repo.remotes(),
            repo.log(&query),
        );
        let err = |e: gf_core::GfError| e.to_string();
        Ok(Arc::new(Self {
            status: status.map_err(err)?,
            branches: branches.map_err(err)?,
            tags: tags.map_err(err)?,
            remotes: remotes.map_err(err)?,
            commits: commits.map_err(err)?,
        }))
    }
}

#[derive(Debug)]
pub struct RepoTab {
    pub repo: Repository,
    pub snapshot: Option<Arc<Snapshot>>,
    pub loading: bool,
    pub error: Option<String>,
    pub sidebar: sidebar::State,
    pub changes: changes::State,
    pub history: history::State,
}

impl RepoTab {
    pub fn new(repo: Repository) -> Self {
        Self {
            repo,
            snapshot: None,
            loading: true,
            error: None,
            sidebar: sidebar::State::default(),
            changes: changes::State::default(),
            history: history::State::default(),
        }
    }

    pub fn name(&self) -> String {
        self.repo.handle().display_name()
    }

    /// Current branch name, or `HEAD` (short id) when detached.
    pub fn head_label(&self) -> Option<String> {
        let head = &self.snapshot.as_ref()?.status.head;
        Some(match (&head.branch, &head.oid) {
            (Some(b), _) => b.clone(),
            (None, Some(oid)) => format!("HEAD@{}", oid.short()),
            (None, None) => "(no commits)".into(),
        })
    }
}
