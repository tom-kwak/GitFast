//! Running git work for the UI: loading data, generic operations with
//! result reporting, and network operations with progress and cancel.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gf_core::model::{Oid, RepoId};
use gf_git::{CancellationToken, GitCommand, PullMode, Repository, RunOptions};
use iced::Task;
use iced::futures::SinkExt;

use super::dialogs::{Confirm, Purpose};
use super::{App, Checkout, CheckoutPhase, Message, OpKind};
use crate::features::command_log::NoteKind;
use crate::features::dialog::Dialog;
use crate::features::diff_view::DiffSource;
use crate::features::history::file_search::{self, FilterJob, Work};
use crate::features::history::search;
use crate::features::toolbar;
use crate::git_actions;
use crate::repo_tab::{History, Parts, RepoTab, Snapshot, StageUpdate, Touched};
use crate::undo::Step;
use crate::widgets::format::truncate;
use gf_git::{Capture, HeadMode, Scope};

/// Pending clicks belong to the repository so switching or closing its tab
/// cannot drop a queued index write.
#[derive(Debug)]
pub(super) struct FileStages {
    repo: Repository,
    current: Option<(PathBuf, bool)>,
    waiting: VecDeque<(PathBuf, bool)>,
}

impl FileStages {
    pub(super) fn restore(&self, tab: &mut RepoTab) {
        tab.stage_update = Some(StageUpdate::Running);
        for (path, checked) in self.current.iter().chain(&self.waiting) {
            tab.changes.set_pending_stage(path.clone(), *checked);
        }
    }
}

/// Reads the tab's queued parts. Only one load runs per tab: each starts
/// from the last snapshot, so a load finishing after a newer one would put
/// old parts back. Only [`App::load_due`] calls this; everything else
/// queues parts with [`App::request_parts`].
fn load_snapshot(tab: &mut RepoTab, checkouts: &mut HashMap<PathBuf, Checkout>) -> Task<Message> {
    // A read already in flight when git finished cannot release the checkout guard.
    if let Some(checkout) = checkouts.get_mut(&tab.repo.handle().git_dir)
        && checkout.phase == CheckoutPhase::RefreshQueued
    {
        checkout.phase = CheckoutPhase::Refreshing(tab.repo.id());
    }
    let parts = std::mem::take(&mut tab.queued);
    tab.loading_touched = std::mem::take(&mut tab.touched);
    tab.loading = true;
    let repo = tab.repo.clone();
    let id = repo.id();
    let all = tab.all_branches;
    let limit = tab.history.head_limit();
    let prev = tab.snapshot.clone();
    Task::perform(
        Snapshot::load(repo, prev, parts, all, limit),
        move |result| Message::Loaded(id, result),
    )
}

/// Remotes of `snap` this session has not listed the tags of yet.
fn remotes_to_list(snap: &Snapshot, listed: &std::collections::HashSet<String>) -> Vec<String> {
    snap.remotes
        .iter()
        .map(|r| r.name.clone())
        .filter(|n| !listed.contains(n))
        .collect()
}

/// Lists the tags of `remotes` (network, read-only) and reloads the refs
/// so the push count includes unpublished tags. A remote that cannot be
/// reached stays unlisted: its tags are listed at the next fetch.
fn list_remote_tags(repo: Repository, remotes: Vec<String>) -> Task<Message> {
    let id = repo.id();
    Task::perform(
        async move {
            for remote in remotes {
                if let Err(e) = repo.record_remote_tags(&remote).await {
                    tracing::warn!(%remote, error = %e, "could not list remote tags");
                }
            }
        },
        move |()| Message::Core(gf_core::CoreEvent::RefsChanged(id)),
    )
}

/// Writes a missing commit-graph (git logs the command, a failure only
/// leaves the history as fast as it was).
fn write_commit_graph(repo: Repository) -> Task<Message> {
    Task::future(async move {
        if let Err(e) = repo.write_commit_graph_if_missing().await {
            tracing::warn!(error = %e, "could not write a commit-graph");
        }
    })
    .discard()
}

fn file_search_work(id: RepoId, repo: Repository, work: Work) -> Task<Message> {
    match work {
        Work::Load(request) => {
            let oid = request.oid;
            let request = request.request;
            let result_oid = oid.clone();
            Task::perform(
                async move {
                    let paths = repo
                        .commit_file_paths(&oid)
                        .await
                        .map_err(|error| error.to_string())?;
                    tokio::task::spawn_blocking(move || {
                        Arc::new(file_search::Index::prepare(paths))
                    })
                    .await
                    .map_err(|error| error.to_string())
                },
                move |result| {
                    Message::FileSearchPathsLoaded(id, request, result_oid.clone(), result)
                },
            )
        }
        Work::Filter(job) => file_search_filter(id, job),
    }
}

fn file_search_filter(id: RepoId, job: FilterJob) -> Task<Message> {
    let request = job.request;
    let generation = job.generation;
    let oid = job.oid.clone();
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || job.index.filter(&job.query))
                .await
                .unwrap_or_else(|error| {
                    tracing::error!(%error, "file search filter failed");
                    Vec::new()
                })
        },
        move |hits| Message::FileSearchFiltered(id, request, generation, oid.clone(), hits),
    )
}

impl App {
    pub(super) fn open_file_search(&mut self) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        let oid = tab
            .snapshot
            .as_ref()
            .and_then(|snap| tab.history.file_search_base(snap));
        let repo = tab.repo.clone();
        let id = repo.id();
        let work = tab.history.open_file_search(oid);
        let task = work.map_or_else(Task::none, |work| file_search_work(id, repo, work));
        Task::batch([task, iced::widget::operation::focus(search::INPUT_ID)])
    }

    /// ⌘F in the main window: the History panel's Find. Brings History to
    /// the front; with the search already open it only takes the keyboard.
    pub(super) fn find_file_shortcut(&mut self) -> Task<Message> {
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        if tab.history.file_search_is_open() {
            return iced::widget::operation::focus(search::INPUT_ID);
        }
        // A file history has no tree of its own to search: `×` goes back first.
        if tab.history.file_revision.is_some() {
            self.flash(
                NoteKind::Info,
                "close this file history (×) to find another file",
            );
            return Task::none();
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.code_view = crate::features::diff_view::Role::Commit;
        }
        self.expand_history();
        self.open_file_search()
    }

    pub(super) fn query_file_search(&mut self, query: String) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        let id = tab.repo.id();
        tab.history
            .file_search_query(query)
            .map_or_else(Task::none, |job| file_search_filter(id, job))
    }

    pub(super) fn close_file_search(&mut self) -> Task<Message> {
        if let Some(tab) = self.active_tab_mut() {
            tab.history.close_file_search();
        }
        self.restore_focus()
    }

    /// Mouse and ⏎ both land here: opens the picked file's history from
    /// the commit its candidates were read from.
    pub(super) fn activate_file_search(&mut self, row: usize) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        let Some((path, oid)) = tab.history.file_search_selection(row) else {
            return Task::none();
        };
        let origin = tab.history.revision().map(str::to_owned);
        let open = self.open_file_history_from_search(path, oid, origin);
        Task::batch([open, self.restore_focus()])
    }

    pub(super) fn file_search_paths_loaded(
        &mut self,
        id: RepoId,
        request: u64,
        oid: Oid,
        result: Result<Arc<file_search::Index>, String>,
    ) -> Task<Message> {
        // Any tab: `State::paths_loaded` drops results for a closed search, so
        // a tab left mid-load is not stuck on `Loading files…`.
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        let Some(job) = tab.history.file_search_paths_loaded(request, &oid, result) else {
            return Task::none();
        };
        file_search_filter(id, job)
    }

    pub(super) fn file_search_filtered(
        &mut self,
        id: RepoId,
        request: u64,
        generation: u64,
        oid: Oid,
        hits: Vec<file_search::Hit>,
    ) -> Task<Message> {
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        let completion = tab
            .history
            .file_search_filter_finished(request, generation, &oid, hits);
        let next = completion
            .next
            .map_or_else(Task::none, |job| file_search_filter(id, job));
        if completion.accepted {
            // New rows start at the top, like the highlight.
            return Task::batch([next, search::State::scroll_to_top()]);
        }
        next
    }
}

pub fn load_history(tab: &mut RepoTab, revision: String) -> Task<Message> {
    let repo = tab.repo.clone();
    let id = repo.id();
    let limit = tab.history.revision_limit();
    tab.history_request = tab.history_request.wrapping_add(1);
    let request = tab.history_request;
    let file_revision = tab.history.file_revision.clone();
    Task::perform(
        {
            let revision = revision.clone();
            async move {
                let history = if let Some((path, start)) = file_revision {
                    History::load_path(&repo, path, start.map(|oid| oid.to_string()), limit).await
                } else {
                    History::load(&repo, vec![revision], false, limit).await
                };
                history.map(Arc::new).map_err(|e| e.to_string())
            }
        },
        move |result| Message::HistoryLoaded(id, request, revision.clone(), result),
    )
}

pub fn load_diff(tab: &mut RepoTab, source: DiffSource) -> Task<Message> {
    let repo = tab.repo.clone();
    let id = repo.id();
    let role = source.role();
    tab.diff_mut(role)
        .request(repo, source)
        .map(move |done| Message::DiffFinished(id, role, done))
}

impl App {
    /// Refresh after an operation: everything (the history only if its
    /// refs moved).
    pub(super) fn request_refresh(&mut self, id: RepoId) -> Task<Message> {
        self.request_parts(id, Parts::ALL)
    }

    /// Reads `parts` again (any file may have changed); see
    /// [`Self::request_touched`].
    pub(super) fn request_parts(&mut self, id: RepoId, parts: Parts) -> Task<Message> {
        self.request_touched(id, parts, Touched::all())
    }

    /// Queues `parts` for reading because `touched` changed, and starts
    /// whatever load is due. Requests coalesce: while a load runs (or the
    /// tab is in the background) they wait in `RepoTab::queued`.
    pub(super) fn request_touched(
        &mut self,
        id: RepoId,
        parts: Parts,
        touched: Touched,
    ) -> Task<Message> {
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        tab.queued |= parts;
        tab.touched |= touched;
        self.load_due()
    }

    /// Starts the loads that are due (see [`RepoTab::load_due`]). Runs after
    /// every message, so a tab that comes to the front reads what it missed
    /// in the background, however it was brought there.
    ///
    /// A tab that has never loaded (restored at startup) waits while any
    /// other tab loads: the shown tab first, then the others one at a time,
    /// so they do not compete with it for the disk and the CPU.
    pub(super) fn load_due(&mut self) -> Task<Message> {
        let active = self.active;
        let mut busy = false;
        let mut tasks = Vec::new();
        if let Some(tab) = self.tabs.get_mut(active)
            && tab.load_due(true)
        {
            tasks.push(load_snapshot(tab, &mut self.checkouts));
        }
        busy |= self.tabs.iter().any(|t| t.loading);
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if i == active || !tab.load_due(false) {
                continue;
            }
            if tab.snapshot.is_none() {
                if busy {
                    continue;
                }
                busy = true;
            }
            tasks.push(load_snapshot(tab, &mut self.checkouts));
        }
        Task::batch(tasks)
    }

    /// What changed work tree files need read again: the status, and the
    /// submodules when a file in one of them (or `.gitmodules`,
    /// `.gitattributes`) changed. No paths means too many to list.
    pub(super) fn worktree_parts(&self, id: RepoId, paths: &[PathBuf]) -> Parts {
        let subs = self
            .tabs
            .iter()
            .find(|t| t.repo.id() == id)
            .and_then(|t| t.snapshot.as_ref())
            .map(|s| s.submodules.as_slice())
            .unwrap_or_default();
        let modules = paths.is_empty()
            || paths.iter().any(|p| {
                p.as_path() == std::path::Path::new(".gitmodules")
                    || p.as_path() == std::path::Path::new(".gitattributes")
                    || subs.iter().any(|s| p.starts_with(&s.path))
            });
        Parts {
            modules,
            ..Parts::STATUS
        }
    }

    /// ⌘R: everything, the history included.
    pub(super) fn refresh_active(&mut self) -> Task<Message> {
        match self.active_tab_mut() {
            Some(tab) => {
                tab.refresh_asked = true;
                let id = tab.repo.id();
                self.request_parts(id, Parts::RELOAD)
            }
            None => Task::none(),
        }
    }

    pub(super) fn on_loaded(
        &mut self,
        id: RepoId,
        result: Result<Arc<Snapshot>, String>,
    ) -> Task<Message> {
        if let Some(tab) = self.tabs.iter().find(|tab| tab.repo.id() == id)
            && self
                .checkouts
                .get(&tab.repo.handle().git_dir)
                .is_some_and(|c| c.phase == CheckoutPhase::Refreshing(id))
        {
            self.checkouts.remove(&tab.repo.handle().git_dir);
        }
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        tab.loading = false;
        if tab.stage_update == Some(StageUpdate::Refreshing)
            && !(tab.queued.status || tab.queued.refs || tab.queued.config)
        {
            tab.stage_update = None;
            tab.changes.clear_pending_stage();
        }
        // The refresh asked for is done once no read is left queued.
        let refreshed = tab.queued.is_empty() && std::mem::take(&mut tab.refresh_asked);
        let touched = std::mem::take(&mut tab.loading_touched);
        // A page requested while this load ran arrives with the next one.
        if tab.queued.is_empty() {
            tab.history.snapshot_loaded();
        }
        let mut tasks = Vec::new();
        let mut reveal = false;
        match result {
            Ok(snap) => {
                if !tab.sidebar.retain_existing(&snap) && tab.history.revision().is_some() {
                    tab.history.set_revision(None);
                }
                // A file that moved between groups keeps its diff visible.
                let followed = tab.changes.follow(&snap.status.entries);
                // Keep an open conflict resolver in sync: reload it while the
                // file is still conflicted, close it once resolved.
                let conflict_path = tab
                    .conflict
                    .as_ref()
                    .filter(|_| snap.loaded.status)
                    .map(|c| c.path.clone());
                if let Some(path) = conflict_path {
                    let still = snap.status.entries.iter().any(|e| {
                        matches!(e, gf_core::model::StatusEntry::Conflicted { path: p, .. } if *p == path)
                    });
                    if still {
                        let repo = tab.repo.clone();
                        let rid = repo.id();
                        tasks.push(Task::perform(
                            {
                                let path = path.clone();
                                async move {
                                    repo.conflict_segments(&path)
                                        .await
                                        .map_err(|e| e.to_string())
                                }
                            },
                            move |result| Message::ConflictLoaded(rid, path.clone(), result),
                        ));
                    } else {
                        tab.conflict = None;
                    }
                }
                // A merge / rebase gf started and that stopped on
                // conflicts has ended some other way (commit box, terminal,
                // abort): continuing a later one must not record it. A
                // Continue already running holds its own copy.
                if snap.state.is_clean() {
                    tab.undo.set_pending(None, &tab.repo.handle().git_dir);
                }
                // A commit waiting for a longer history (`reveal`).
                reveal = snap.loaded.history;
                // A long history (a full page) loads much faster with a
                // commit-graph: write one in the background if missing.
                if !tab.graph_checked && snap.history.more {
                    tab.graph_checked = true;
                    tasks.push(write_commit_graph(tab.repo.clone()));
                }
                // Which tags push would publish needs every remote's tag
                // list. Each remote is listed once a session, in the
                // background: a missing list, or one gone stale since (a
                // tag pushed from a terminal, another machine).
                let unlisted = remotes_to_list(&snap, &tab.remote_tags_listed);
                if !unlisted.is_empty() {
                    tab.remote_tags_listed.extend(unlisted.iter().cloned());
                    tasks.push(list_remote_tags(tab.repo.clone(), unlisted));
                }
                // The branch shown instead of HEAD only moves with refs.
                let refs = snap.loaded.refs;
                let status = snap.loaded.status;
                if refs {
                    tab.worktrees.set_worktrees(&snap.worktrees);
                }
                tab.snapshot = Some(snap);
                tab.error = None;
                if let Some(rev) = tab.history.revision().map(str::to_owned)
                    && refs
                {
                    tasks.push(load_history(tab, rev));
                }
                // Work tree diffs may have changed on disk: read the shown
                // one again if it moved between groups or its file changed
                // (not on every saved file).
                let stale = tab
                    .local_diff
                    .source()
                    .filter(|_| status)
                    .and_then(|s| match s {
                        DiffSource::WorkTree(p)
                        | DiffSource::Staged(p)
                        | DiffSource::Untracked(p) => touched.includes(p).then(|| s.clone()),
                        _ => None,
                    });
                if let Some(source) = followed.or(stale) {
                    tasks.push(load_diff(tab, source));
                }
                if refreshed {
                    self.flash(NoteKind::Info, "↻ refreshed");
                }
            }
            Err(e) => {
                tab.error = Some(e.clone());
                self.log.note(NoteKind::Error, "refresh failed", vec![e]);
            }
        }
        tasks.push(self.load_due());
        if reveal {
            tasks.push(self.reveal_pending(id));
        }
        // Commits and branch moves in any tree show up as ref changes here.
        tasks.push(self.request_worktrees(id));
        Task::batch(tasks)
    }

    /// Reads the working trees' statuses for tab `id` when its worktrees
    /// view is on screen; nothing otherwise (see `worktrees::State`).
    pub(super) fn request_worktrees(&mut self, id: RepoId) -> Task<Message> {
        if !self.settings.appearance.show_refs
            || self.active_tab().is_none_or(|tab| tab.repo.id() != id)
        {
            return Task::none();
        }
        if self.refs_mode() != crate::features::worktrees::RefsMode::Worktrees {
            return Task::none();
        }
        let open: Vec<PathBuf> = self
            .tabs
            .iter()
            .map(|t| t.repo.cwd().to_path_buf())
            .collect();
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        let here = tab.repo.cwd().to_path_buf();
        tab.worktrees.set_tabs(open, here);
        tab.worktrees
            .request(&tab.repo)
            .map(move |m| Message::WorktreesOf(id, m))
    }

    /// Runs a repository operation; reports and refreshes when done.
    pub(super) fn op<F, Fut>(
        &mut self,
        label: impl Into<String>,
        kind: OpKind,
        f: F,
    ) -> Task<Message>
    where
        F: FnOnce(Repository) -> Fut,
        Fut: Future<Output = gf_core::Result<()>> + Send + 'static,
    {
        self.op_recorded(label, kind, None, f)
    }

    /// [`Self::op`] that records an undo step: `record`'s scope is
    /// captured before and after the operation (see `crate::undo`).
    pub(super) fn op_recorded<F, Fut>(
        &mut self,
        label: impl Into<String>,
        kind: OpKind,
        record: Option<Record>,
        f: F,
    ) -> Task<Message>
    where
        F: FnOnce(Repository) -> Fut,
        Fut: Future<Output = gf_core::Result<()>> + Send + 'static,
    {
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        Self::recorded_task(tab.repo.clone(), label, kind, record, f)
    }

    fn recorded_task<F, Fut>(
        repo: Repository,
        label: impl Into<String>,
        kind: OpKind,
        record: Option<Record>,
        f: F,
    ) -> Task<Message>
    where
        F: FnOnce(Repository) -> Fut,
        Fut: Future<Output = gf_core::Result<()>> + Send + 'static,
    {
        let id = repo.id();
        let label = label.into();
        let fut = f(repo.clone());
        let step_label = label.clone();
        Task::perform(
            async move {
                let before = match record {
                    Some(r) => match before_or_refuse(&repo, r).await {
                        Ok(b) => b,
                        Err(e) => return (Err(e), None),
                    },
                    None => None,
                };
                let result = fut.await.map_err(|e| e.to_string());
                let step = match before {
                    Some(b) => finish(&repo, step_label, b, result.is_ok()).await,
                    None => None,
                };
                (result, step)
            },
            move |(result, step)| Message::OpDone {
                repo: id,
                label: label.clone(),
                kind: kind.clone(),
                result,
                step,
            },
        )
    }

    /// Keep the commit box and checkboxes in sync while writing the index.
    pub(super) fn stage_op<F, Fut>(
        &mut self,
        label: impl Into<String>,
        record: Option<Record>,
        f: F,
    ) -> Task<Message>
    where
        F: FnOnce(Repository) -> Fut,
        Fut: Future<Output = gf_core::Result<()>> + Send + 'static,
    {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        if tab.stage_update.is_some() || tab.commit.is_committing() {
            self.flash(NoteKind::Info, "wait for staged changes to update");
            return Task::none();
        }
        let git_dir = tab.repo.handle().git_dir.clone();
        tab.stage_update = Some(StageUpdate::Running);
        self.op_recorded(
            label,
            OpKind::Stage {
                git_dir,
                file: None,
            },
            record,
            f,
        )
    }

    /// Reflect each click immediately and serialize the actual Git index writes.
    pub(super) fn queue_stage_files(
        &mut self,
        paths: Vec<PathBuf>,
        checked: bool,
    ) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        if tab.commit.is_committing() || paths.is_empty() {
            return Task::none();
        }
        let running = tab.stage_update == Some(StageUpdate::Running);
        let repo = tab.repo.clone();
        let git_dir = repo.handle().git_dir.clone();
        for path in &paths {
            tab.changes.set_pending_stage(path.clone(), checked);
        }
        tab.stage_update = Some(StageUpdate::Running);
        let queue = self
            .stage_files
            .entry(git_dir.clone())
            .or_insert_with(|| FileStages {
                repo,
                current: None,
                waiting: VecDeque::new(),
            });
        queue
            .waiting
            .extend(paths.into_iter().map(|path| (path, checked)));
        if running {
            Task::none()
        } else {
            self.next_file_stage(&git_dir).unwrap_or_else(Task::none)
        }
    }

    fn next_file_stage(&mut self, git_dir: &Path) -> Option<Task<Message>> {
        let queue = self.stage_files.get_mut(git_dir)?;
        let Some((path, checked)) = queue.waiting.pop_front() else {
            self.stage_files.remove(git_dir);
            return None;
        };
        queue.current = Some((path.clone(), checked));
        let label = format!(
            "{} {}",
            if checked { "stage" } else { "unstage" },
            path.display()
        );
        let kind = OpKind::Stage {
            git_dir: git_dir.to_path_buf(),
            file: Some(path.clone()),
        };
        let paths = vec![path];
        let record = Record::new(crate::undo::index(paths.clone()));
        Some(Self::recorded_task(
            queue.repo.clone(),
            label,
            kind,
            Some(record),
            move |r| async move {
                if checked {
                    r.stage(&paths).await
                } else {
                    r.unstage(&paths).await
                }
            },
        ))
    }

    /// Network operation with live progress and cancel (Esc).
    pub(super) fn sync_op<F, Fut>(
        &mut self,
        label: impl Into<String>,
        kind: OpKind,
        f: F,
    ) -> Task<Message>
    where
        F: FnOnce(Repository, RunOptions) -> Fut + Send + 'static,
        Fut: Future<Output = gf_core::Result<()>> + Send + 'static,
    {
        if self.sync.is_some() {
            self.flash(NoteKind::Error, "another fetch/pull/push is still running");
            return Task::none();
        }
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        let repo = tab.repo.clone();
        let id = repo.id();
        let label = label.into();
        // A pull moves the branch; let it be undone.
        let record = matches!(kind, OpKind::Pull(_)).then(|| Record::head(HeadMode::Keep));
        let step_label = label.clone();
        let token = CancellationToken::new();
        self.sync = Some((
            toolbar::Sync {
                label: label.clone(),
                progress: None,
            },
            token.clone(),
        ));

        let stream = iced::stream::channel(
            64,
            async move |mut out: iced::futures::channel::mpsc::Sender<Message>| {
                let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel();
                let before = match record {
                    Some(r) => before_or_refuse(&repo, r).await.ok().flatten(),
                    None => None,
                };
                let fut = f(
                    repo.clone(),
                    RunOptions {
                        cancel: Some(token),
                        progress: Some(ptx),
                        stop_at: None,
                    },
                );
                tokio::pin!(fut);
                let result = loop {
                    tokio::select! {
                        Some(p) = prx.recv() => {
                            let _ = out.send(Message::SyncProgress(p)).await;
                        }
                        r = &mut fut => break r,
                    }
                };
                let ok = result.is_ok();
                let step = match before {
                    Some(b) => finish(&repo, step_label, b, ok).await,
                    None => None,
                };
                let _ = out
                    .send(Message::OpDone {
                        repo: id,
                        label,
                        kind,
                        result: result.map_err(|e| e.to_string()),
                        step,
                    })
                    .await;
            },
        );
        Task::run(stream, |m| m)
    }

    pub(super) fn checkout_pending(&self) -> bool {
        self.active_tab()
            .is_some_and(|tab| self.checkouts.contains_key(&tab.repo.handle().git_dir))
    }

    /// Reserve the worktree before launching any part of the switch.
    pub(super) fn begin_checkout(
        &mut self,
        target: String,
        phase: CheckoutPhase,
    ) -> Option<PathBuf> {
        let git_dir = self.active_tab()?.repo.handle().git_dir.clone();
        let std::collections::hash_map::Entry::Vacant(entry) =
            self.checkouts.entry(git_dir.clone())
        else {
            return None;
        };
        entry.insert(Checkout { target, phase });
        Some(git_dir)
    }

    pub(super) fn checkout_op<F, Fut>(
        &mut self,
        target: String,
        label: impl Into<String>,
        record: Record,
        f: F,
    ) -> Task<Message>
    where
        F: FnOnce(Repository) -> Fut,
        Fut: Future<Output = gf_core::Result<()>> + Send + 'static,
    {
        let Some(git_dir) = self.begin_checkout(target, CheckoutPhase::Running) else {
            return Task::none();
        };
        self.op_recorded(label, OpKind::Checkout(git_dir), Some(record), f)
    }

    pub(super) fn on_op_done(
        &mut self,
        id: RepoId,
        label: String,
        kind: OpKind,
        result: Result<(), String>,
        recorded: bool,
    ) -> Task<Message> {
        let next_stage = if let OpKind::Stage { git_dir, file } = &kind {
            if result.is_err()
                && let Some(path) = file
                && self
                    .stage_files
                    .get(git_dir)
                    .is_none_or(|queue| !queue.waiting.iter().any(|(queued, _)| queued == path))
                && let Some(tab) = self.tab_mut(id)
            {
                tab.changes.forget_pending_stage(path);
            }
            self.next_file_stage(git_dir)
        } else {
            None
        };
        if matches!(kind, OpKind::Stage { .. } | OpKind::Commit)
            && next_stage.is_none()
            && let Some(tab) = self.tab_mut(id)
        {
            tab.stage_update = Some(StageUpdate::Refreshing);
        }
        if matches!(kind, OpKind::Fetch | OpKind::Push { .. } | OpKind::Pull(_)) {
            self.sync = None;
        }
        if let OpKind::Checkout(git_dir) = &kind {
            if result.is_ok() && self.tabs.iter().any(|tab| tab.repo.id() == id) {
                if let Some(checkout) = self.checkouts.get_mut(git_dir) {
                    checkout.phase = CheckoutPhase::RefreshQueued;
                }
            } else {
                self.checkouts.remove(git_dir);
            }
        }
        let refresh = if let Some(next) = next_stage {
            next
        } else if matches!(kind, OpKind::Stage { .. }) {
            self.request_parts(id, Parts::STATUS)
        } else if let OpKind::DeleteBranches(git_dir) = &kind {
            self.deleting_branches.remove(git_dir);
            self.request_touched(
                id,
                Parts {
                    refs: true,
                    config: true,
                    ..Parts::default()
                },
                Touched::default(),
            )
        } else {
            self.request_refresh(id)
        };
        let active = self.active_tab().is_some_and(|tab| tab.repo.id() == id);
        let mut tasks = vec![refresh];
        // Close the tabs of the trees that are gone, also when a cleanup
        // stopped part-way.
        if let OpKind::RemoveWorktrees(paths) = &kind {
            // Tabs keep resolved paths: resolve the (still existing) parent.
            let gone: Vec<PathBuf> = paths
                .iter()
                .filter(|p| !p.exists())
                .flat_map(|p| {
                    let resolved = p
                        .parent()
                        .and_then(crate::platform::canonicalize)
                        .zip(p.file_name())
                        .map(|(d, n)| d.join(n));
                    std::iter::once(p.clone()).chain(resolved)
                })
                .collect();
            // A removed tree is not a repository to come back to: drop it
            // from the recent list too (welcome page and open window).
            let before = self.settings.repos.recent.len();
            self.settings
                .repos
                .recent
                .retain(|r| !gone.iter().any(|g| r == g));
            if self.settings.repos.recent.len() != before {
                self.save_settings();
                for g in &gone {
                    if let Some(p) = self.picker.as_mut() {
                        p.forgotten(g);
                    }
                }
            }
            for i in (0..self.tabs.len()).rev() {
                if gone.iter().any(|p| self.tabs[i].repo.cwd().starts_with(p)) {
                    tasks.push(self.close_tab(i));
                }
            }
        }
        if let OpKind::Undo { redo } = kind {
            tasks.push(self.on_undo_done(id, redo, result.is_ok()));
        }
        match result {
            Ok(()) => {
                if active && !matches!(kind, OpKind::Undo { .. }) {
                    let hint = if recorded { " · ⌘Z undo" } else { "" };
                    self.flash(NoteKind::Info, format!("✓ {}{hint}", truncate(&label, 60)));
                }
                if let OpKind::AddWorktree { path, open: true } = &kind {
                    tasks.push(self.update(Message::Open(path.clone())));
                }
                if let Some(tab) = self.tab_mut(id)
                    && kind == OpKind::Commit
                {
                    tab.commit.committed();
                }
            }
            Err(e) => {
                if kind == OpKind::Commit
                    && let Some(tab) = self.tab_mut(id)
                {
                    tab.commit.commit_failed();
                }
                if !active {
                    self.log
                        .note(NoteKind::Error, format!("{label} failed"), vec![e]);
                    return Task::batch(tasks);
                }
                if e.contains("operation cancelled") {
                    self.flash(NoteKind::Info, format!("{label} cancelled"));
                    return Task::batch(tasks);
                }
                // Offer the next step for common push/pull failures;
                // anything else gets the failure report.
                let can_ask = self.dialog.is_none()
                    && self.code_dialog.is_none()
                    && self.rebase_editor().is_none();
                let asked = match &kind {
                    OpKind::Push {
                        branch: Some(branch),
                        to: None,
                        tags,
                        ..
                    } if can_ask && e.contains(gf_git::CONFIGURED_PUSH_REWRITES) => {
                        tasks.push(self.confirm(Confirm::delete(
                            "Push may overwrite remote refs",
                            vec![
                                "Git is configured to force updates, delete refs or mirror the remote.".into(),
                                "Review the remote configuration before continuing.".into(),
                            ],
                            "Push with configured rules",
                            Purpose::Push {
                                repo: id,
                                branch: branch.clone(),
                                to: None,
                                tags: tags.clone(),
                                allow_configured_rewrites: true,
                            },
                        )));
                        true
                    }
                    OpKind::Push {
                        branch,
                        to,
                        offer_force: true,
                        ..
                    } if can_ask
                        && (e.contains("non-fast-forward") || e.contains("fetch first")) =>
                    {
                        // Overwrites commits on the remote: a deletion.
                        let ask = self.confirm(Confirm::delete(
                            "Push rejected",
                            vec![
                                "The remote branch has commits you don't have.".into(),
                                "Usually: pull first, then push again.".into(),
                                "Force-with-lease overwrites the remote branch, but only if".into(),
                                "nobody pushed since your last fetch.".into(),
                            ],
                            "Force push (with lease)",
                            Purpose::ForcePush {
                                branch: branch.clone(),
                                to: to.clone(),
                            },
                        ));
                        tasks.push(ask);
                        true
                    }
                    OpKind::Pull(
                        PullMode::FastForwardOnly | PullMode::FastForwardWithAutostash,
                    ) if can_ask
                        && (e.contains("Not possible to fast-forward")
                            || e.contains("divergent")
                            || e.contains("reconcile")) =>
                    {
                        let ask = self.confirm(
                            Confirm::change(
                                "pull-strategy",
                                "Branches have diverged",
                                vec![
                                    "Your branch and its upstream both have new commits.".into(),
                                    "Choose how to combine them:".into(),
                                ],
                                "Pull",
                                Purpose::PullStrategy(id),
                            )
                            .always()
                            .check(
                                "rebase my commits on top (instead of a merge commit)",
                                false,
                            ),
                        );
                        tasks.push(ask);
                        true
                    }
                    OpKind::Pull(PullMode::FastForwardOnly)
                        if can_ask && e.contains("local changes") && e.contains("overwritten") =>
                    {
                        let ask = self.confirm(
                            Confirm::change(
                                "pull-autostash",
                                "Local changes overlap the pull",
                                vec![
                                    "Git stopped before updating your working tree.".into(),
                                    "Temporarily stash tracked changes, pull, and reapply them? Reapplying can produce conflicts.".into(),
                                ],
                                "Stash and pull",
                                Purpose::Pull { repo: id, mode: PullMode::FastForwardWithAutostash },
                            )
                            .always(),
                        );
                        tasks.push(ask);
                        true
                    }
                    _ => false,
                };
                let mut lines = vec![e.clone()];
                if let Some(hint) = git_actions::hint_for_error(&[], &e) {
                    lines.push(format!("hint: {hint}"));
                }
                self.log
                    .note(NoteKind::Error, format!("{label} failed"), lines);
                let hint = if recorded {
                    " · ⌘Z undo"
                } else {
                    " — see log"
                };
                self.flash(NoteKind::Error, format!("✗ {}{hint}", truncate(&label, 40)));
                if !asked {
                    tasks.push(self.show_failure(&label, &e));
                }
            }
        }
        if active {
            tasks.push(self.restore_focus());
        }
        Task::batch(tasks)
    }

    /// Reports a failed git command in a dialog: git's own words, what
    /// most likely happened and what to do (see [`git_actions::diagnose`]).
    /// Left out while another dialog or the rebase editor is open, so it
    /// never replaces a question; the command log still has the details.
    pub(super) fn show_failure(&mut self, label: &str, error: &str) -> Task<Message> {
        if self.dialog.is_some() || self.code_dialog.is_some() || self.rebase_editor().is_some() {
            return Task::none();
        }
        let d = git_actions::diagnose(error);
        let mut body = vec![format!("{} failed — git said:", truncate(label, 60))];
        body.extend(
            git_actions::error_excerpt(error, 6)
                .into_iter()
                .map(|l| format!("  {l}")),
        );
        body.push(String::new());
        body.push(format!("What happened: {}", d.cause));
        body.push(String::new());
        body.push("What to do:".into());
        body.extend(
            d.steps
                .iter()
                .enumerate()
                .map(|(i, s)| format!("  {}. {s}", i + 1)),
        );
        self.open_dialog(Dialog::notice(d.title, body, Purpose::Failure))
    }

    pub(super) fn run_git(&mut self, args: Vec<String>) -> Task<Message> {
        let tab = self.active_tab();
        // clone/init run next to the current repository (or in $HOME) and
        // open the result; everything else runs in the active repository.
        let (cwd, repo) = match tab {
            Some(tab) if !git_actions::creates_repository(&args) => {
                (tab.repo.cwd().to_path_buf(), Some(tab.repo.id()))
            }
            Some(tab) => (
                tab.repo
                    .cwd()
                    .parent()
                    .map_or_else(home_dir, std::path::Path::to_path_buf),
                None,
            ),
            None => (home_dir(), None),
        };
        if tab.is_none() && !git_actions::creates_repository(&args) && !is_global_command(&args) {
            // Without a repository the command would run in the home folder.
            self.flash(
                NoteKind::Error,
                "no repository open — ⌘O to open one, or `clone <url>`",
            );
            return Task::none();
        }
        let open_after = git_actions::created_repository(&args, &cwd);
        let mut cmd = GitCommand::new(&args).current_dir(&cwd).echo_stdout();
        if let Some(id) = repo {
            cmd = cmd.repo(id);
        }
        let runner = self.runner.clone();
        if !self.settings.appearance.show_command_log {
            self.flash(NoteKind::Info, "running… output in the command log (⌘J)");
        }
        Task::perform(
            async move {
                runner
                    .run(&cmd)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            },
            move |result| Message::GitFinished {
                repo,
                args: args.clone(),
                result,
                open_after: open_after.clone(),
            },
        )
    }

    pub(super) fn on_git_finished(
        &mut self,
        repo: Option<RepoId>,
        args: Vec<String>,
        result: Result<(), String>,
        open_after: Option<PathBuf>,
    ) -> Task<Message> {
        let title = format!("git {}", args.join(" "));
        let mut tasks = Vec::new();
        match result {
            Ok(()) => {
                self.flash(NoteKind::Info, format!("✓ {}", truncate(&title, 50)));
                if let Some(dir) = open_after {
                    tasks.push(self.update(Message::Open(dir)));
                }
            }
            Err(e) => {
                if let Some(hint) = git_actions::hint_for_error(&args, &e) {
                    self.log.note(
                        NoteKind::Info,
                        "what to do next",
                        vec![format!("hint: {hint}")],
                    );
                }
                self.flash(
                    NoteKind::Error,
                    format!("✗ {} — see log", truncate(&title, 40)),
                );
                tasks.push(self.show_failure(&title, &e));
            }
        }
        if let Some(id) = repo {
            tasks.push(self.request_refresh(id));
        }
        Task::batch(tasks)
    }
}

fn home_dir() -> PathBuf {
    crate::platform::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Commands that make sense without a repository.
fn is_global_command(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some("--version" | "version" | "help" | "config" | "ls-remote")
    )
}

/// What an operation touches, for its undo step (see `crate::undo`).
#[derive(Debug, Clone)]
pub struct Record {
    pub scope: Scope,
    /// How HEAD moves back when the branch stays the same.
    pub mode: HeadMode,
    /// Commit message to put back in the commit box on undo.
    pub message: Option<String>,
    /// Applying / popping this stash entry: its files join the scope.
    pub stash_entry: Option<usize>,
    /// The changed files, read fresh when the operation starts, join the
    /// scope (the snapshot may be older than the last edit).
    pub changes: Option<Changes>,
    /// The confirmation promised ⌘Z: without a saved copy the operation
    /// does not run.
    pub required: bool,
}

/// Which changed files join a [`Record`]'s scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Changes {
    /// Changes to tracked files and conflicts (what a hard reset throws away).
    Tracked,
    /// Untracked files too (stash).
    All,
}

impl Record {
    pub fn new(scope: Scope) -> Self {
        Self {
            scope,
            mode: HeadMode::Keep,
            message: None,
            stash_entry: None,
            changes: None,
            required: false,
        }
    }

    /// Moves HEAD only (commit, reset, merge, checkout…).
    pub fn head(mode: HeadMode) -> Self {
        Self {
            mode,
            ..Self::new(Scope::head())
        }
    }

    pub fn with_message(mut self, message: String) -> Self {
        self.message = Some(message);
        self
    }

    pub fn with_stash_entry(mut self, index: usize) -> Self {
        self.stash_entry = Some(index);
        self
    }

    pub fn with_changes(mut self, changes: Changes) -> Self {
        self.changes = Some(changes);
        self
    }

    /// Refuse to run without a copy saved for undo.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
}

/// [`capture_before`], or the error the operation fails with when the
/// record is required. `Ok(None)`: run without an undo step.
async fn before_or_refuse(
    repo: &Repository,
    record: Record,
) -> Result<Option<(Record, Capture)>, String> {
    let required = record.required;
    match capture_before(repo, record).await {
        Ok(b) => Ok(Some(b)),
        Err(e) if required => Err(format!(
            "nothing was changed: couldn't keep a copy for undo ({e})"
        )),
        Err(e) => {
            tracing::info!(error = %e, "no undo step for this operation");
            Ok(None)
        }
    }
}

/// Captures `record`'s scope before an operation. Fails when there is too
/// much to save or git could not be asked.
pub async fn capture_before(
    repo: &Repository,
    mut record: Record,
) -> Result<(Record, Capture), String> {
    if let Some(i) = record.stash_entry {
        let paths = repo.stash_paths(i).await.map_err(|e| e.to_string())?;
        record.scope.index.extend(paths.iter().cloned());
        record.scope.files.extend(paths);
    }
    if let Some(changes) = record.changes {
        let status = repo.status().await.map_err(|e| e.to_string())?;
        let paths: Vec<_> = crate::undo::changed_paths(&status)
            .into_iter()
            .filter(|p| {
                changes == Changes::All
                    || !status.entries.iter().any(|e| {
                        matches!(e, gf_core::model::StatusEntry::Untracked { path } if path == p)
                    })
            })
            .collect();
        record.scope.index.extend(paths.iter().cloned());
        record.scope.files.extend(paths);
    }
    // Folders (e.g. an untracked one) stand for the files in them, for
    // the index as well as for the work tree.
    let mut index = Scope {
        files: std::mem::take(&mut record.scope.index),
        ..Scope::default()
    };
    index
        .expand_files(repo.cwd())
        .and_then(|()| record.scope.expand_files(repo.cwd()))
        .map_err(|e| match e {
            gf_core::GfError::Unsupported(_) => {
                "too much to keep: over 64 MB or 5000 files".to_owned()
            }
            e => e.to_string(),
        })?;
    record.scope.index = index.files;
    let before = repo
        .capture(&record.scope)
        .await
        .map_err(|e| e.to_string())?;
    Ok((record, before))
}

/// The step, once the operation ran; `None` when it changed nothing.
/// A failed operation that moved HEAD is not recorded: it stopped half
/// way (e.g. a pull that hit conflicts mid-rebase), and undoing to or
/// from that state would only confuse.
pub async fn finish(
    repo: &Repository,
    label: String,
    (record, before): (Record, Capture),
    succeeded: bool,
) -> Option<Box<Step>> {
    let after = repo.capture(&record.scope).await.ok()?;
    if after == before {
        return None;
    }
    let mut step = Step::new(label, record.scope, before, after, record.mode);
    step.message = record.message;
    (succeeded || !step.moves_head()).then(|| Box::new(step))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gf_git::GitRunner;
    use gf_test_support::TestRepo;
    use std::path::PathBuf;

    async fn open(repo: &TestRepo) -> Repository {
        let runner = GitRunner::new().with_cleared_env().with_env(repo.env());
        Repository::open(repo.path(), runner)
            .await
            .expect("open repository")
    }

    /// Restored tabs: the shown one loads first, the others one at a
    /// time after it; a background tab's saved files wait until it is shown.
    #[tokio::test]
    async fn the_shown_tab_loads_first_and_background_tabs_wait() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.commit_all("init");
        let home = tempfile::tempdir().expect("home");
        let (mut app, _) = App::boot(
            gf_config::AppPaths::under(home.path()),
            gf_config::Settings::default(),
        );
        for _ in 0..3 {
            app.tabs.push(RepoTab::new(open(&repo).await, None));
        }
        app.active = 1;
        let loading = |app: &App| app.tabs.iter().map(|t| t.loading).collect::<Vec<_>>();

        let _ = app.load_due();
        assert_eq!(loading(&app), [false, true, false], "the shown tab alone");
        let _ = app.load_due();
        assert_eq!(
            loading(&app),
            [false, true, false],
            "nothing more meanwhile"
        );

        // The shown tab's load finishes: one background tab starts.
        let id = app.tabs[1].repo.id();
        let _ = app.on_loaded(id, Ok(Arc::default()));
        assert_eq!(loading(&app), [true, false, false]);
        let id = app.tabs[0].repo.id();
        let _ = app.on_loaded(id, Ok(Arc::default()));
        assert_eq!(loading(&app), [false, false, true]);
        let id = app.tabs[2].repo.id();
        let _ = app.on_loaded(id, Ok(Arc::default()));
        assert_eq!(loading(&app), [false, false, false]);

        // A saved file in a background tab waits for the tab to be shown.
        let id = app.tabs[0].repo.id();
        let _ = app.request_touched(id, Parts::STATUS, Touched::paths(&["a.txt".into()]));
        assert_eq!(loading(&app), [false, false, false]);
        assert_eq!(app.tabs[0].queued, Parts::STATUS);
        app.active = 0;
        let _ = app.load_due();
        assert_eq!(loading(&app), [true, false, false], "read once shown");
        assert!(
            app.tabs[0]
                .loading_touched
                .includes(std::path::Path::new("a.txt"))
        );
    }

    #[tokio::test]
    async fn a_hard_reset_saves_tracked_changes_read_when_it_starts() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.commit_all("init");
        repo.write("a.txt", "edited after the last refresh\n");
        repo.write("scratch.log", "untracked\n");
        let r = open(&repo).await;
        let record = Record::new(Scope::head()).with_changes(Changes::Tracked);
        let (record, before) = capture_before(&r, record).await.expect("captured");
        assert_eq!(record.scope.files, [PathBuf::from("a.txt")]);
        assert_eq!(before.files.len(), 1);
        let all = Record::new(Scope::default()).with_changes(Changes::All);
        let (all, _) = capture_before(&r, all).await.expect("captured");
        assert_eq!(all.scope.files.len(), 2, "stash also keeps untracked files");
    }

    #[tokio::test]
    async fn a_required_copy_that_cannot_be_kept_stops_the_operation() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.commit_all("init");
        let r = open(&repo).await;
        // A stash entry that does not exist cannot be read.
        let record = Record::new(Scope::default()).with_stash_entry(7);
        assert!(matches!(
            before_or_refuse(&r, record.clone()).await,
            Ok(None)
        ));
        let err = before_or_refuse(&r, record.required())
            .await
            .expect_err("refused");
        assert!(err.starts_with("nothing was changed"), "{err}");
    }
}
