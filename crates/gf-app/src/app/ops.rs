//! Running git work for the UI: loading data, generic operations with
//! result reporting, and network operations with progress and cancel.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use gf_core::model::RepoId;
use gf_git::{CancellationToken, GitCommand, Repository, RunOptions};
use iced::Task;
use iced::futures::SinkExt;

use super::dialogs::{Confirm, Purpose};
use super::{App, Message, OpKind};
use crate::features::command_log::NoteKind;
use crate::features::dialog::Dialog;
use crate::features::diff_view::{self, DiffSource};
use crate::features::toolbar;
use crate::git_actions;
use crate::repo_tab::{History, Parts, RepoTab, Snapshot};
use crate::undo::Step;
use crate::widgets::format::truncate;
use gf_git::{Capture, HeadMode, Scope};

/// Reads `parts` of the tab's repository (plus any queued ones). Only
/// one load runs per tab: each starts from the last snapshot, so a load
/// finishing after a newer one would put old parts back. Go through
/// `App::request_parts` unless the tab is new.
pub fn load_snapshot(tab: &mut RepoTab, parts: Parts) -> Task<Message> {
    let parts = parts | std::mem::take(&mut tab.queued);
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

pub fn load_history(tab: &RepoTab, revision: String) -> Task<Message> {
    let repo = tab.repo.clone();
    let id = repo.id();
    let limit = tab.history.revision_limit();
    Task::perform(
        {
            let revision = revision.clone();
            async move {
                // `file: <path>` is a file history (see `open_file_history`).
                let history = match revision.strip_prefix("file: ") {
                    Some(path) => History::load_path(&repo, path.into(), limit).await,
                    None => History::load(&repo, vec![revision], false, limit).await,
                };
                history.map(Arc::new).map_err(|e| e.to_string())
            }
        },
        move |result| Message::HistoryLoaded(id, revision.clone(), result),
    )
}

pub fn load_diff(tab: &RepoTab, role: diff_view::Role) -> Task<Message> {
    let view = tab.diff(role);
    let Some(source) = view.source().cloned() else {
        return Task::none();
    };
    let repo = tab.repo.clone();
    let id = repo.id();
    let ws = view.ignore_whitespace();
    Task::perform(diff_view::load(repo, source.clone(), ws), move |result| {
        Message::DiffLoaded(id, source.clone(), result)
    })
}

impl App {
    /// Refresh after an operation: everything (the history only if its
    /// refs moved).
    pub(super) fn request_refresh(&mut self, id: RepoId) -> Task<Message> {
        self.request_parts(id, Parts::ALL)
    }

    /// Reads `parts` again; coalesced while a load is running.
    pub(super) fn request_parts(&mut self, id: RepoId, parts: Parts) -> Task<Message> {
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        if tab.loading {
            tab.queued |= parts;
            Task::none()
        } else {
            load_snapshot(tab, parts)
        }
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
        match self.active_tab().map(|t| t.repo.id()) {
            Some(id) => self.request_parts(id, Parts::RELOAD),
            None => Task::none(),
        }
    }

    pub(super) fn on_loaded(
        &mut self,
        id: RepoId,
        result: Result<Arc<Snapshot>, String>,
    ) -> Task<Message> {
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        tab.loading = false;
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
                if let Some(source) = tab.changes.follow(&snap.status.entries) {
                    tab.local_diff.show(source);
                }
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
                    tab.pending = None;
                }
                // A commit waiting for a longer history (`reveal`).
                reveal = snap.loaded.history;
                // The branch shown instead of HEAD only moves with refs.
                let refs = snap.loaded.refs;
                let status = snap.loaded.status;
                tab.snapshot = Some(snap);
                tab.error = None;
                if let Some(rev) = tab.history.revision().map(str::to_owned) {
                    if refs {
                        tasks.push(load_history(tab, rev));
                    }
                }
                // Work tree diffs may have changed on disk.
                if let Some(
                    source @ (DiffSource::WorkTree(_)
                    | DiffSource::Staged(_)
                    | DiffSource::Untracked(_)),
                ) = tab.local_diff.source().cloned().filter(|_| status)
                {
                    tab.local_diff.show(source);
                    tasks.push(load_diff(tab, diff_view::Role::Local));
                }
            }
            Err(e) => {
                tab.error = Some(e.clone());
                self.log.note(NoteKind::Error, "refresh failed", vec![e]);
            }
        }
        if let Some(tab) = self.tab_mut(id) {
            if !tab.queued.is_empty() {
                tasks.push(load_snapshot(tab, Parts::default()));
            }
        }
        if reveal {
            tasks.push(self.reveal_pending(id));
        }
        Task::batch(tasks)
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
        let repo = tab.repo.clone();
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
        let record = (kind == OpKind::Pull).then(|| Record::head(HeadMode::Keep));
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

    pub(super) fn on_op_done(
        &mut self,
        id: RepoId,
        label: String,
        kind: OpKind,
        result: Result<(), String>,
        recorded: bool,
    ) -> Task<Message> {
        if matches!(kind, OpKind::Fetch | OpKind::Push { .. } | OpKind::Pull) {
            self.sync = None;
        }
        let mut tasks = vec![self.request_refresh(id)];
        if let OpKind::Undo { redo } = kind {
            tasks.push(self.on_undo_done(id, redo, result.is_ok()));
        }
        match result {
            Ok(()) => {
                if !matches!(kind, OpKind::Undo { .. }) {
                    let hint = if recorded { " · ⌘Z undo" } else { "" };
                    self.flash(NoteKind::Info, format!("✓ {}{hint}", truncate(&label, 60)));
                }
                if let Some(tab) = self.tab_mut(id) {
                    if kind == OpKind::Commit {
                        tab.commit.committed();
                    }
                }
            }
            Err(e) => {
                if kind == OpKind::Commit {
                    if let Some(tab) = self.tab_mut(id) {
                        tab.commit.commit_failed();
                    }
                }
                if e.contains("operation cancelled") {
                    self.flash(NoteKind::Info, format!("{label} cancelled"));
                    return Task::batch(tasks);
                }
                // Offer the next step for common push/pull failures;
                // anything else gets the failure report.
                let asked = match &kind {
                    OpKind::Push { branch, to }
                        if e.contains("non-fast-forward")
                            || e.contains("fetch first")
                            || e.contains("[rejected]") =>
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
                    OpKind::Pull if e.contains("divergent") || e.contains("reconcile") => {
                        let ask = self.confirm(
                            Confirm::change(
                                "pull-strategy",
                                "Branches have diverged",
                                vec![
                                    "Your branch and its upstream both have new commits.".into(),
                                    "Choose how to combine them:".into(),
                                ],
                                "Pull",
                                Purpose::PullStrategy,
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
                    _ => false,
                };
                let mut lines = vec![e.clone()];
                if let Some(hint) = git_actions::hint_for_error(&[], &e) {
                    lines.push(format!("hint: {hint}"));
                }
                self.log
                    .note(NoteKind::Error, format!("{label} failed"), lines);
                self.flash(
                    NoteKind::Error,
                    format!("✗ {} — see log", truncate(&label, 40)),
                );
                self.settings.appearance.show_command_log = true;
                if !asked {
                    tasks.push(self.show_failure(&label, &e));
                }
            }
        }
        tasks.push(self.restore_focus());
        Task::batch(tasks)
    }

    /// Reports a failed git command in a dialog: git's own words, what
    /// most likely happened and what to do (see [`git_actions::diagnose`]).
    /// Left out while another dialog or the rebase editor is open, so it
    /// never replaces a question; the command log still has the details.
    pub(super) fn show_failure(&mut self, label: &str, error: &str) -> Task<Message> {
        if self.dialog.is_some() || self.rebase_editor.is_some() {
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
                self.settings.appearance.show_command_log = true;
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
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
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
