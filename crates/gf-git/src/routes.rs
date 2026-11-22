//! Which backend serves each git command. `Repository`'s public methods are
//! the UI's API; each one calls the backend chosen here (see the table in
//! `docs/GIX_MIGRATION.md`):
//!
//! - `XxxOps::x(&self.cli, …)` — the git CLI.
//! - `gix_then_cli!(self, XxxOps::x(…))` — gix first; the git CLI when gix
//!   is off, does not support the request, or fails.

#![allow(clippy::wildcard_imports)]

use std::path::{Path, PathBuf};

use gf_core::Result;
use gf_core::model::{Branch, Commit, FileDiff, Oid, Remote, StashEntry, StatusReport, Tag};

use crate::Repository;
use crate::backend::*;
use crate::cli::RunOptions;
use crate::conflict::{Choice, Segment};
use crate::ops::bisect::{BisectProgress, Verdict};
use crate::ops::blame::BlameLine;
use crate::ops::commit::CommitOptions;
use crate::ops::conflict::Side;
use crate::ops::diff::{DiffOptions, DiffTarget, FileVersion};
use crate::ops::integrate::{Integration, MergeStyle};
use crate::ops::lfs::LfsFile;
use crate::ops::log::LogQuery;
use crate::ops::rebase_interactive::{SquashPreview, TodoItem};
use crate::ops::reflog::{ReflogEntry, ResetMode};
use crate::ops::stash::StashOptions;
use crate::ops::state::RepoState;
use crate::ops::submodule::Submodule;
use crate::ops::sync::{PullMode, PushOptions};
use crate::ops::undo::{Capture, HeadMode, Scope};
use crate::ops::worktree::Worktree;

/// Calls `Trait::method(args)` on gix when it is available, else (or when
/// gix answers `Unsupported` or fails) on the git CLI.
macro_rules! gix_then_cli {
    ($self:ident, $tr:ident :: $m:ident ( $($arg:expr),* )) => {{
        #[cfg(feature = "gix")]
        if let Some(gix) = &$self.gix {
            match $tr::$m(gix, $($arg),*).await {
                Ok(v) => return Ok(v),
                Err(gf_core::GfError::Unsupported(what)) => {
                    tracing::trace!(what, concat!(stringify!($m), ": not with gix, using git CLI"));
                }
                Err(e) => {
                    tracing::warn!(error = %e, concat!(stringify!($m), ": gix failed, using git CLI"));
                }
            }
        }
        $tr::$m(&$self.cli, $($arg),*).await
    }};
}

impl Repository {
    // ---- LogOps (gix first)
    /// Paths present in the tree of `commit`.
    pub async fn commit_file_paths(&self, commit: &Oid) -> Result<Vec<PathBuf>> {
        self.cli.commit_file_paths(commit).await
    }

    /// Commit history.
    pub async fn log(&self, query: &LogQuery) -> Result<Vec<Commit>> {
        gix_then_cli!(self, LogOps::log(query))
    }

    /// `log` through gix alone, to check it against [`Self::log_cli`].
    /// `None` when gix is off or could not open the repository.
    pub async fn log_fast(&self, query: &LogQuery) -> Option<Result<Vec<Commit>>> {
        #[cfg(feature = "gix")]
        if let Some(gix) = &self.gix {
            return Some(LogOps::log(gix, query).await);
        }
        let _ = query;
        None
    }

    /// `log` through the git CLI only, to check the gix path against it.
    pub async fn log_cli(&self, query: &LogQuery) -> Result<Vec<Commit>> {
        LogOps::log(&self.cli, query).await
    }

    // ---- BisectOps
    /// Starts bisecting between a known bad and a known good revision.
    pub async fn bisect_start(&self, bad: &str, good: &str) -> Result<BisectProgress> {
        BisectOps::bisect_start(&self.cli, bad, good).await
    }
    /// Marks the checked-out commit and moves to the next candidate.
    pub async fn bisect_mark(&self, verdict: Verdict) -> Result<BisectProgress> {
        BisectOps::bisect_mark(&self.cli, verdict).await
    }
    /// Ends bisecting and returns to the original branch.
    pub async fn bisect_reset(&self) -> Result<()> {
        BisectOps::bisect_reset(&self.cli).await
    }

    // ---- BlameOps
    /// Blame of `path` at `rev` (`None` = the working tree version).
    pub async fn blame(&self, path: &Path, rev: Option<&str>) -> Result<Vec<BlameLine>> {
        BlameOps::blame(&self.cli, path, rev).await
    }

    // ---- BranchOps
    /// Validates a branch name with git's own rules. Returns a readable
    /// reason when invalid.
    pub async fn check_branch_name(&self, name: &str) -> Result<Option<String>> {
        BranchOps::check_branch_name(&self.cli, name).await
    }
    /// Creates `name` at `start` (default HEAD), optionally switching to it.
    pub async fn create_branch(
        &self,
        name: &str,
        start: Option<&str>,
        checkout: bool,
    ) -> Result<()> {
        gix_then_cli!(self, BranchOps::create_branch(name, start, checkout))
    }
    /// Switches to a local branch. For a remote branch short name such as
    /// `origin/feature`, creates and switches to a tracking branch `feature`.
    pub async fn switch_branch(&self, name: &str, remote: bool) -> Result<()> {
        BranchOps::switch_branch(&self.cli, name, remote).await
    }
    /// Changed `paths` that keep `git switch` from moving to `target` (it
    /// refuses to overwrite them). Empty: the changes can come along.
    pub async fn switch_blockers(&self, target: &str, paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let differing = self.cli.paths_differing_from_head(target).await?;
        Ok(crate::ops::branch::switch_blockers(paths, &differing))
    }
    /// Stashes every change (untracked files too) as
    /// [`left_on`](crate::left_on)`(from)`, then switches like
    /// [`Repository::switch_branch`]. When the switch fails the changes are
    /// put back, so nothing is left half done.
    pub async fn stash_and_switch(&self, from: &str, name: &str, remote: bool) -> Result<()> {
        let message = crate::left_on(from);
        let opts = StashOptions {
            message: Some(message.clone()),
            include_untracked: true,
            keep_index: false,
            paths: Vec::new(),
        };
        StashOps::stash_push(&self.cli, &opts).await?;
        // `git stash` saves nothing (and says so) when there was nothing to
        // save: only an entry made here may be popped back.
        let made = StashOps::stashes(&self.cli)
            .await?
            .first()
            .is_some_and(|s| s.message.ends_with(&message));
        if let Err(e) = BranchOps::switch_branch(&self.cli, name, remote).await {
            if made && let Err(back) = StashOps::stash_pop(&self.cli, 0).await {
                return Err(gf_core::GfError::Other(format!(
                    "{e}\nyour changes are kept in stash@{{0}} ({back})"
                )));
            }
            return Err(e);
        }
        Ok(())
    }
    /// Adds `pattern` as a line of the work tree's `.gitignore` (made when
    /// missing), unless it is there already.
    pub async fn ignore(&self, pattern: &str) -> Result<()> {
        let file = self.cwd().join(".gitignore");
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        if text.lines().any(|l| l.trim() == pattern) {
            return Ok(());
        }
        let mut out = text;
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(pattern);
        out.push('\n');
        std::fs::write(&file, out)?;
        Ok(())
    }
    /// Row of `oid` in the history `query` lists (0 = newest), found by
    /// reading commit ids until it comes up; `None` when it is not in that
    /// history. Limit, skip and filters of `query` are ignored.
    pub async fn log_position(&self, query: &LogQuery, oid: &Oid) -> Result<Option<usize>> {
        self.cli.log_position(query, oid.as_str()).await
    }
    /// Checks out a commit without a branch (detached HEAD).
    pub async fn checkout_detached(&self, rev: &str) -> Result<()> {
        BranchOps::checkout_detached(&self.cli, rev).await
    }
    /// Deletes a local branch. Without `force`, git refuses when the branch
    /// is not merged, which protects work.
    pub async fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        gix_then_cli!(self, BranchOps::delete_branch(name, force))
    }
    /// Deletes several local branches with normal Git merge and worktree checks.
    pub async fn delete_branches(&self, names: &[String], force: bool) -> Result<()> {
        if let [name] = names {
            self.delete_branch(name, force).await
        } else {
            BranchOps::delete_branches(&self.cli, names, force).await
        }
    }

    pub async fn rename_branch(&self, old: &str, new: &str) -> Result<()> {
        BranchOps::rename_branch(&self.cli, old, new).await
    }
    /// Deletes a branch on a remote (`git push <remote> --delete <branch>`).
    pub async fn delete_remote_branch(&self, remote: &str, branch: &str) -> Result<()> {
        BranchOps::delete_remote_branch(&self.cli, remote, branch).await
    }

    /// Deletes several heads on one remote.
    pub async fn delete_remote_branches(&self, remote: &str, branches: &[String]) -> Result<()> {
        BranchOps::delete_remote_branches(&self.cli, remote, branches).await
    }

    // ---- CommitOps
    /// Commits the index with `message` (passed on stdin, so any text is
    /// safe). Returns the new commit id. Hooks run as usual. With `amend`, an
    /// empty message keeps the last commit's message.
    pub async fn commit(&self, message: &str, opts: &CommitOptions) -> Result<Oid> {
        CommitOps::commit(&self.cli, message, opts).await
    }
    /// Full message of HEAD, e.g. to prefill an amend. `None` before the
    /// first commit.
    pub async fn head_message(&self) -> Result<Option<String>> {
        CommitOps::head_message(&self.cli).await
    }

    // ---- ConflictOps
    /// The conflicted file split into common text and conflict blocks.
    /// `None` when the file has no parseable markers (binary, or edited).
    pub async fn conflict_segments(&self, path: &Path) -> Result<Option<Vec<Segment>>> {
        ConflictOps::conflict_segments(&self.cli, path).await
    }
    /// Writes the file with `choices` applied (unchosen blocks keep their
    /// markers) and stages it when no conflict is left.
    /// Returns how many conflicts remain.
    pub async fn resolve_blocks(
        &self,
        path: &Path,
        segments: &[Segment],
        choices: &[Option<Choice>],
    ) -> Result<usize> {
        ConflictOps::resolve_blocks(&self.cli, path, segments, choices).await
    }
    /// Resolves the whole file with one side and stages it.
    pub async fn take_side(&self, path: &Path, side: Side) -> Result<()> {
        ConflictOps::take_side(&self.cli, path, side).await
    }

    // ---- DiffOps
    /// Reads the full file from the requested version, up to the preview size limit.
    pub async fn file_bytes(&self, path: &Path, version: &FileVersion) -> Result<Vec<u8>> {
        DiffOps::file_bytes(&self.cli, path, version).await
    }

    pub async fn diff(&self, target: &DiffTarget, opts: &DiffOptions) -> Result<Vec<FileDiff>> {
        DiffOps::diff(&self.cli, target, opts).await
    }
    /// Added/removed line counts per changed file: unstaged changes, or
    /// staged ones with `staged`. `None` per file means binary.
    pub async fn numstat(
        &self,
        staged: bool,
    ) -> Result<std::collections::HashMap<PathBuf, Option<crate::cli::parse::numstat::LineCounts>>>
    {
        gix_then_cli!(self, DiffOps::numstat(staged))
    }

    // ---- IndexOps
    /// Stages the given paths, including deletions (`git add -A -- paths`).
    pub async fn stage(&self, paths: &[PathBuf]) -> Result<()> {
        IndexOps::stage(&self.cli, paths).await
    }
    /// Stages every change in the working tree.
    pub async fn stage_all(&self) -> Result<()> {
        IndexOps::stage_all(&self.cli).await
    }
    /// Removes the given paths from the index, keeping working tree content.
    pub async fn unstage(&self, paths: &[PathBuf]) -> Result<()> {
        IndexOps::unstage(&self.cli, paths).await
    }
    pub async fn unstage_all(&self) -> Result<()> {
        IndexOps::unstage_all(&self.cli).await
    }
    /// Throws away unstaged changes to tracked files and deletes the given
    /// untracked files. Irreversible: callers must confirm with the user.
    pub async fn discard(&self, tracked: &[PathBuf], untracked: &[PathBuf]) -> Result<()> {
        IndexOps::discard(&self.cli, tracked, untracked).await
    }
    /// Applies a patch to the index (`cached`) or the working tree.
    /// `reverse` un-applies it. Used for hunk and line staging with patches
    /// from [`crate::patch`].
    pub async fn apply_patch(&self, patch: String, cached: bool, reverse: bool) -> Result<()> {
        IndexOps::apply_patch(&self.cli, patch, cached, reverse).await
    }

    // ---- IntegrateOps
    pub async fn conflicted_count(&self) -> Result<usize> {
        IntegrateOps::conflicted_count(&self.cli).await
    }
    /// Merges `rev` into the current branch.
    pub async fn merge(&self, rev: &str, style: MergeStyle) -> Result<Integration> {
        IntegrateOps::merge(&self.cli, rev, style).await
    }
    /// Replays the current branch on top of `onto`.
    pub async fn rebase(&self, onto: &str) -> Result<Integration> {
        IntegrateOps::rebase(&self.cli, onto).await
    }
    /// Applies the given commits (oldest first) on top of HEAD.
    pub async fn cherry_pick(&self, revs: &[String]) -> Result<Integration> {
        IntegrateOps::cherry_pick(&self.cli, revs).await
    }
    /// Creates a commit undoing `rev`.
    pub async fn revert(&self, rev: &str) -> Result<Integration> {
        IntegrateOps::revert(&self.cli, rev).await
    }
    /// Reverts several commits, one new commit each, in the given order
    /// (newest first undoes cleanly).
    pub async fn revert_many(&self, revs: &[String]) -> Result<Integration> {
        IntegrateOps::revert_many(&self.cli, revs).await
    }
    /// Continues the operation in progress after conflicts were resolved and
    /// staged.
    pub async fn continue_operation(&self) -> Result<Integration> {
        IntegrateOps::continue_operation(&self.cli).await
    }
    /// Skips the current commit of a rebase / cherry-pick / revert.
    pub async fn skip_operation(&self) -> Result<Integration> {
        IntegrateOps::skip_operation(&self.cli).await
    }
    /// Abandons the operation and restores the state from before it began.
    pub async fn abort_operation(&self) -> Result<()> {
        IntegrateOps::abort_operation(&self.cli).await
    }

    // ---- LfsOps
    /// Whether `git lfs` is installed.
    // ---- MaintenanceOps
    /// See [`MaintenanceOps::write_commit_graph_if_missing`].
    pub async fn write_commit_graph_if_missing(&self) -> Result<bool> {
        MaintenanceOps::write_commit_graph_if_missing(&self.cli).await
    }

    pub async fn lfs_available(&self) -> bool {
        LfsOps::lfs_available(&self.cli).await
    }
    pub async fn lfs_tracked(&self) -> Result<Vec<String>> {
        LfsOps::lfs_tracked(&self.cli).await
    }
    pub async fn lfs_files(&self) -> Result<Vec<LfsFile>> {
        LfsOps::lfs_files(&self.cli).await
    }
    /// Starts tracking `pattern` with LFS (edits `.gitattributes`).
    pub async fn lfs_track(&self, pattern: &str) -> Result<()> {
        LfsOps::lfs_track(&self.cli, pattern).await
    }
    pub async fn lfs_untrack(&self, pattern: &str) -> Result<()> {
        LfsOps::lfs_untrack(&self.cli, pattern).await
    }
    /// Downloads LFS content for the current checkout.
    pub async fn lfs_pull(&self) -> Result<()> {
        LfsOps::lfs_pull(&self.cli).await
    }

    // ---- LogOps
    /// False on an unborn branch (repository without commits).
    pub async fn head_exists(&self) -> Result<bool> {
        gix_then_cli!(self, LogOps::head_exists())
    }

    // ---- RebaseOps
    /// Commits a rebase onto `base` would replay, oldest first. `None`
    /// base means from the root commit.
    pub async fn rebase_plan(&self, base: Option<&Oid>) -> Result<Vec<TodoItem>> {
        gix_then_cli!(self, RebaseOps::rebase_plan(base))
    }
    /// Checks that `ids` (any order) are adjacent commits of the current
    /// branch and collects what the squash dialog shows.
    pub async fn squash_preview(&self, ids: &[Oid]) -> Result<SquashPreview> {
        gix_then_cli!(self, RebaseOps::squash_preview(ids))
    }
    /// Melds `ids` (adjacent commits of the current branch) into one
    /// commit with `message`. `autostash` stashes uncommitted changes
    /// first and restores them afterwards.
    pub async fn squash(&self, ids: &[Oid], message: &str, autostash: bool) -> Result<Integration> {
        RebaseOps::squash(&self.cli, ids, message, autostash).await
    }
    /// Runs the interactive rebase described by `items` onto `base`
    /// (`None` = rewrite from the root).
    pub async fn rebase_interactive(
        &self,
        base: Option<&Oid>,
        items: &[TodoItem],
    ) -> Result<Integration> {
        RebaseOps::rebase_interactive(&self.cli, base, items).await
    }

    // ---- ReflogOps
    /// Most recent HEAD movements, newest first.
    pub async fn reflog(&self, limit: usize) -> Result<Vec<ReflogEntry>> {
        gix_then_cli!(self, ReflogOps::reflog(limit))
    }
    /// Current HEAD commit, `None` before the first commit.
    pub async fn head_oid(&self) -> Result<Option<Oid>> {
        gix_then_cli!(self, ReflogOps::head_oid())
    }
    /// `git reset --soft|--mixed|--hard <to>`: moves the current branch
    /// (or a detached HEAD) to `to`.
    pub async fn reset(&self, to: &Oid, mode: ResetMode) -> Result<()> {
        ReflogOps::reset(&self.cli, to, mode).await
    }
    /// Moves the current branch back to `to`.
    ///
    /// With `soft`, the undone commits' changes stay staged (undoing a
    /// commit gives the work back); otherwise `reset --keep` restores the
    /// files too but refuses to overwrite uncommitted changes.
    pub async fn undo_to(&self, to: &Oid, soft: bool) -> Result<()> {
        ReflogOps::undo_to(&self.cli, to, soft).await
    }

    // ---- RefOps
    /// Local and remote-tracking branches, with upstream ahead/behind counts.
    pub async fn branches(&self) -> Result<Vec<Branch>> {
        gix_then_cli!(self, RefOps::branches())
    }
    pub async fn tags(&self) -> Result<Vec<Tag>> {
        gix_then_cli!(self, RefOps::tags())
    }
    /// Commits reachable from HEAD that no remote-tracking branch contains:
    /// what a first push of an unpublished branch would upload. `0` before
    /// the first commit.
    pub async fn unpushed_count(&self) -> Result<u32> {
        gix_then_cli!(self, RefOps::unpushed_count())
    }
    /// Commits reachable from HEAD that are on no remote and not reachable
    /// from `since`, newest first.
    pub async fn unpushed(&self, since: Option<&Oid>) -> Result<Vec<Oid>> {
        gix_then_cli!(self, RefOps::unpushed(since))
    }

    /// The newest `limit` of [`Self::unpushed`].
    pub async fn unpushed_recent(&self, limit: usize) -> Result<Vec<Oid>> {
        gix_then_cli!(self, RefOps::unpushed_recent(limit))
    }

    // ---- RemoteOps
    pub async fn remotes(&self) -> Result<Vec<Remote>> {
        gix_then_cli!(self, RemoteOps::remotes())
    }

    // ---- StashOps
    pub async fn stashes(&self) -> Result<Vec<StashEntry>> {
        gix_then_cli!(self, StashOps::stashes())
    }
    /// Saves local changes to a new stash entry.
    pub async fn stash_push(&self, opts: &StashOptions) -> Result<()> {
        StashOps::stash_push(&self.cli, opts).await
    }
    /// Applies a stash, keeping it in the list. `--index` restores what
    /// was staged as staged.
    pub async fn stash_apply(&self, index: usize) -> Result<()> {
        StashOps::stash_apply(&self.cli, index).await
    }
    /// Applies a stash and removes it when that succeeded.
    pub async fn stash_pop(&self, index: usize) -> Result<()> {
        StashOps::stash_pop(&self.cli, index).await
    }
    /// Deletes a stash entry. Irreversible from the UI's point of view.
    pub async fn stash_drop(&self, index: usize) -> Result<()> {
        StashOps::stash_drop(&self.cli, index).await
    }

    // ---- StateOps
    pub async fn state(&self) -> Result<RepoState> {
        StateOps::state(&self.cli).await
    }

    // ---- StatusOps
    /// Working tree and index status plus HEAD/upstream info.
    pub async fn status(&self) -> Result<StatusReport> {
        gix_then_cli!(self, StatusOps::status())
    }

    // ---- SubmoduleOps
    pub async fn submodules(&self) -> Result<Vec<Submodule>> {
        SubmoduleOps::submodules(&self.cli).await
    }
    /// `git submodule update --init --recursive`: checks out the recorded
    /// commits, initializing new submodules.
    pub async fn update_submodules(&self) -> Result<()> {
        SubmoduleOps::update_submodules(&self.cli).await
    }
    /// Re-reads submodule URLs from `.gitmodules`.
    pub async fn sync_submodules(&self) -> Result<()> {
        SubmoduleOps::sync_submodules(&self.cli).await
    }

    // ---- SyncOps
    /// `git fetch --prune` from one remote or all of them.
    /// With `with_gix_fetch(true)`, gix first; a cancel is final, any
    /// other gix error or unsupported case fetches with the CLI.
    pub async fn fetch(&self, remote: Option<&str>, opts: RunOptions) -> Result<()> {
        #[cfg(feature = "gix-fetch")]
        if let Some(gix) = self.gix.as_ref().filter(|_| self.gix_fetch) {
            match SyncOps::fetch(gix, remote, opts.clone()).await {
                Ok(()) => return Ok(()),
                Err(e @ gf_core::GfError::Cancelled) => return Err(e),
                Err(gf_core::GfError::Unsupported(what)) => {
                    tracing::debug!(what, "fetch: not with gix, using git CLI");
                }
                Err(e) => tracing::warn!(error = %e, "fetch: gix failed, using git CLI"),
            }
        }
        SyncOps::fetch(&self.cli, remote, opts).await
    }
    pub async fn pull(&self, mode: PullMode, opts: RunOptions) -> Result<()> {
        SyncOps::pull(&self.cli, mode, opts).await
    }
    pub async fn push(&self, push: &PushOptions, opts: RunOptions) -> Result<()> {
        SyncOps::push(&self.cli, push, opts).await
    }
    pub async fn guard_default_push(&self, branch: &str) -> Result<()> {
        self.cli.guard_default_push(branch).await
    }

    // ---- TagOps
    /// Creates a tag at `target` (default HEAD). With a message the tag is
    /// annotated, otherwise lightweight.
    pub async fn create_tag(
        &self,
        name: &str,
        target: Option<&str>,
        message: Option<&str>,
    ) -> Result<()> {
        gix_then_cli!(self, TagOps::create_tag(name, target, message))
    }
    pub async fn delete_tag(&self, name: &str) -> Result<()> {
        gix_then_cli!(self, TagOps::delete_tag(name))
    }
    pub async fn push_tag(&self, remote: &str, name: &str) -> Result<()> {
        TagOps::push_tag(&self.cli, remote, name).await
    }

    // ---- WorktreeOps
    pub async fn worktrees(&self) -> Result<Vec<Worktree>> {
        gix_then_cli!(self, WorktreeOps::worktrees())
    }
    /// Adds a working tree at `path` for `branch`; with `new_branch`, the
    /// branch is created from HEAD.
    pub async fn add_worktree(&self, path: &Path, branch: &str, new_branch: bool) -> Result<()> {
        WorktreeOps::add_worktree(&self.cli, path, branch, new_branch).await
    }
    /// Removes a linked working tree. Without `force`, git refuses when it
    /// has uncommitted changes.
    pub async fn remove_worktree(&self, path: &Path, force: bool) -> Result<()> {
        WorktreeOps::remove_worktree(&self.cli, path, force).await
    }
    pub async fn prune_worktrees(&self) -> Result<()> {
        WorktreeOps::prune_worktrees(&self.cli).await
    }
}

impl Repository {
    // ---- UndoOps (CLI)
    /// Files a stash entry changes (what applying it touches).
    pub async fn stash_paths(&self, index: usize) -> Result<Vec<PathBuf>> {
        UndoOps::stash_paths(&self.cli, index).await
    }
    /// The state of everything `scope` covers, for undo / redo.
    pub async fn capture(&self, scope: &Scope) -> Result<Capture> {
        UndoOps::capture(&self.cli, scope).await
    }
    /// Puts back a captured state (see `ops::undo`).
    pub async fn restore(
        &self,
        scope: &Scope,
        expected: Option<&Capture>,
        to: &Capture,
        mode: HeadMode,
    ) -> Result<()> {
        UndoOps::restore(&self.cli, scope, expected, to, mode).await
    }
    /// Keeps saved work tree copies from `git gc` (see
    /// [`UndoOps::pin_saved_files`]).
    pub async fn pin_saved_files(&self, blobs: &[String]) -> Result<()> {
        UndoOps::pin_saved_files(&self.cli, blobs).await
    }
    /// Commits leaving `branch` (moved `from` → `to`) that its upstream
    /// already has (see [`UndoOps::published_leaving`]).
    pub async fn published_leaving(
        &self,
        branch: &str,
        from: &str,
        to: Option<&str>,
    ) -> Result<usize> {
        UndoOps::published_leaving(&self.cli, branch, from, to).await
    }
}
