//! The git interface: one trait per area (branches, log, status…), each
//! implemented by a backend. [`CliBackend`] runs the system `git` and
//! implements every trait; gix-based implementations are added area by area
//! (see `docs/GIX_MIGRATION.md`). Which backend serves each command is
//! decided in `routes.rs`.
//!
//! Methods return `impl Future + Send` so implementations can be written as
//! `async fn` and still be awaited from the UI's `Send` tasks.

mod cli;
#[cfg(feature = "gix")]
mod gix_backend;
#[cfg(feature = "gix-fetch")]
mod gix_fetch;
#[cfg(feature = "gix")]
mod gix_refs;
#[cfg(feature = "gix")]
mod gix_status;

use std::path::{Path, PathBuf};

use gf_core::Result;
use gf_core::model::{Branch, Commit, FileDiff, Oid, Remote, StashEntry, StatusReport, Tag};

pub use cli::CliBackend;
#[cfg(feature = "gix")]
pub use gix_backend::GixBackend;

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

/// `git bisect`: find the commit that introduced a bug.
pub trait BisectOps {
    /// Starts bisecting between a known bad and a known good revision.
    fn bisect_start(
        &self,
        bad: &str,
        good: &str,
    ) -> impl Future<Output = Result<BisectProgress>> + Send;

    /// Marks the checked-out commit and moves to the next candidate.
    fn bisect_mark(&self, verdict: Verdict) -> impl Future<Output = Result<BisectProgress>> + Send;

    /// Ends bisecting and returns to the original branch.
    fn bisect_reset(&self) -> impl Future<Output = Result<()>> + Send;
}

/// Line-by-line authorship.
pub trait BlameOps {
    /// Blame of `path` at `rev` (`None` = the working tree version).
    fn blame(
        &self,
        path: &Path,
        rev: Option<&str>,
    ) -> impl Future<Output = Result<Vec<BlameLine>>> + Send;
}

/// Local and remote branches: create, switch, rename, delete.
pub trait BranchOps {
    /// Validates a branch name with git's own rules. Returns a readable
    /// reason when invalid.
    fn check_branch_name(&self, name: &str) -> impl Future<Output = Result<Option<String>>> + Send;

    /// Creates `name` at `start` (default HEAD), optionally switching to it.
    fn create_branch(
        &self,
        name: &str,
        start: Option<&str>,
        checkout: bool,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Switches to a local branch. For a remote branch short name such as
    /// `origin/feature`, creates and switches to a tracking branch `feature`.
    fn switch_branch(&self, name: &str, remote: bool) -> impl Future<Output = Result<()>> + Send;

    /// Checks out a commit without a branch (detached HEAD).
    fn checkout_detached(&self, rev: &str) -> impl Future<Output = Result<()>> + Send;

    /// Deletes a local branch. Without `force`, git refuses when the branch
    /// is not merged, which protects work.
    fn delete_branch(&self, name: &str, force: bool) -> impl Future<Output = Result<()>> + Send;

    /// Deletes local branches in one operation.
    fn delete_branches(
        &self,
        names: &[String],
        force: bool,
    ) -> impl Future<Output = Result<()>> + Send;

    fn rename_branch(&self, old: &str, new: &str) -> impl Future<Output = Result<()>> + Send;

    /// Deletes a branch on a remote (`git push <remote> --delete <branch>`).
    fn delete_remote_branch(
        &self,
        remote: &str,
        branch: &str,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Deletes several heads in a single push to the same remote.
    fn delete_remote_branches(
        &self,
        remote: &str,
        branches: &[String],
    ) -> impl Future<Output = Result<()>> + Send;
}

/// Creating commits.
pub trait CommitOps {
    /// Commits the index with `message` (passed on stdin, so any text is
    /// safe). Returns the new commit id. Hooks run as usual. With `amend`, an
    /// empty message keeps the last commit's message.
    fn commit(
        &self,
        message: &str,
        opts: &CommitOptions,
    ) -> impl Future<Output = Result<Oid>> + Send;

    /// Full message of HEAD, e.g. to prefill an amend. `None` before the
    /// first commit.
    fn head_message(&self) -> impl Future<Output = Result<Option<String>>> + Send;
}

/// Resolving merge conflicts in the working tree.
pub trait ConflictOps {
    /// The conflicted file split into common text and conflict blocks.
    /// `None` when the file has no parseable markers (binary, or edited).
    fn conflict_segments(
        &self,
        path: &Path,
    ) -> impl Future<Output = Result<Option<Vec<Segment>>>> + Send;

    /// Writes the file with `choices` applied (unchosen blocks keep their
    /// markers) and stages it when no conflict is left.
    /// Returns how many conflicts remain.
    fn resolve_blocks(
        &self,
        path: &Path,
        segments: &[Segment],
        choices: &[Option<Choice>],
    ) -> impl Future<Output = Result<usize>> + Send;

    /// Resolves the whole file with one side and stages it.
    fn take_side(&self, path: &Path, side: Side) -> impl Future<Output = Result<()>> + Send;
}

/// Diffs and per-file line counts.
pub trait DiffOps {
    /// Reads the full file from the requested version, up to the preview size limit.
    fn file_bytes(
        &self,
        path: &Path,
        version: &FileVersion,
    ) -> impl Future<Output = Result<Vec<u8>>> + Send;

    fn diff(
        &self,
        target: &DiffTarget,
        opts: &DiffOptions,
    ) -> impl Future<Output = Result<Vec<FileDiff>>> + Send;

    /// Added/removed line counts per changed file: unstaged changes, or
    /// staged ones with `staged`. `None` per file means binary.
    fn numstat(
        &self,
        staged: bool,
    ) -> impl Future<
        Output = Result<
            std::collections::HashMap<PathBuf, Option<crate::cli::parse::numstat::LineCounts>>,
        >,
    > + Send;
}

/// The index (staging area) and discarding changes.
pub trait IndexOps {
    /// Stages the given paths, including deletions (`git add -A -- paths`).
    fn stage(&self, paths: &[PathBuf]) -> impl Future<Output = Result<()>> + Send;

    /// Stages every change in the working tree.
    fn stage_all(&self) -> impl Future<Output = Result<()>> + Send;

    /// Removes the given paths from the index, keeping working tree content.
    fn unstage(&self, paths: &[PathBuf]) -> impl Future<Output = Result<()>> + Send;

    fn unstage_all(&self) -> impl Future<Output = Result<()>> + Send;

    /// Throws away unstaged changes to tracked files and deletes the given
    /// untracked files. Irreversible: callers must confirm with the user.
    fn discard(
        &self,
        tracked: &[PathBuf],
        untracked: &[PathBuf],
    ) -> impl Future<Output = Result<()>> + Send;

    /// Applies a patch to the index (`cached`) or the working tree.
    /// `reverse` un-applies it. Used for hunk and line staging with patches
    /// from [`crate::patch`].
    fn apply_patch(
        &self,
        patch: String,
        cached: bool,
        reverse: bool,
    ) -> impl Future<Output = Result<()>> + Send;
}

/// Merge, rebase, cherry-pick, revert and the operations in progress.
pub trait IntegrateOps {
    fn conflicted_count(&self) -> impl Future<Output = Result<usize>> + Send;

    /// Merges `rev` into the current branch.
    fn merge(
        &self,
        rev: &str,
        style: MergeStyle,
    ) -> impl Future<Output = Result<Integration>> + Send;

    /// Replays the current branch on top of `onto`.
    fn rebase(&self, onto: &str) -> impl Future<Output = Result<Integration>> + Send;

    /// Applies the given commits (oldest first) on top of HEAD.
    fn cherry_pick(&self, revs: &[String]) -> impl Future<Output = Result<Integration>> + Send;

    /// Creates a commit undoing `rev`.
    fn revert(&self, rev: &str) -> impl Future<Output = Result<Integration>> + Send;

    /// Reverts several commits, one new commit each, in the given order
    /// (newest first undoes cleanly).
    fn revert_many(&self, revs: &[String]) -> impl Future<Output = Result<Integration>> + Send;

    /// Continues the operation in progress after conflicts were resolved and
    /// staged.
    fn continue_operation(&self) -> impl Future<Output = Result<Integration>> + Send;

    /// Skips the current commit of a rebase / cherry-pick / revert.
    fn skip_operation(&self) -> impl Future<Output = Result<Integration>> + Send;

    /// Abandons the operation and restores the state from before it began.
    fn abort_operation(&self) -> impl Future<Output = Result<()>> + Send;
}

/// Git LFS.
/// Housekeeping that speeds up later reads (see `ops/maintenance.rs`).
pub trait MaintenanceOps {
    /// Writes the commit-graph file when the repository has none (and is
    /// not shallow, and `core.commitGraph` is not off). `true` when written.
    fn write_commit_graph_if_missing(&self) -> impl Future<Output = Result<bool>> + Send;
}

pub trait LfsOps {
    /// Whether `git lfs` is installed.
    fn lfs_available(&self) -> impl Future<Output = bool> + Send;

    fn lfs_tracked(&self) -> impl Future<Output = Result<Vec<String>>> + Send;

    fn lfs_files(&self) -> impl Future<Output = Result<Vec<LfsFile>>> + Send;

    /// Starts tracking `pattern` with LFS (edits `.gitattributes`).
    fn lfs_track(&self, pattern: &str) -> impl Future<Output = Result<()>> + Send;

    fn lfs_untrack(&self, pattern: &str) -> impl Future<Output = Result<()>> + Send;

    /// Downloads LFS content for the current checkout.
    fn lfs_pull(&self) -> impl Future<Output = Result<()>> + Send;
}

/// Commit history.
pub trait LogOps {
    /// `git log` for `query`. An unborn branch is an empty history.
    fn log(&self, query: &LogQuery) -> impl Future<Output = Result<Vec<Commit>>> + Send;

    /// False on an unborn branch (repository without commits).
    fn head_exists(&self) -> impl Future<Output = Result<bool>> + Send;
}

/// Interactive rebase and squash.
pub trait RebaseOps {
    /// Commits a rebase onto `base` would replay, oldest first. `None`
    /// base means from the root commit.
    fn rebase_plan(&self, base: Option<&Oid>)
    -> impl Future<Output = Result<Vec<TodoItem>>> + Send;

    /// Checks that `ids` (any order) are adjacent commits of the current
    /// branch and collects what the squash dialog shows.
    fn squash_preview(&self, ids: &[Oid]) -> impl Future<Output = Result<SquashPreview>> + Send;

    /// Melds `ids` (adjacent commits of the current branch) into one
    /// commit with `message`. `autostash` stashes uncommitted changes
    /// first and restores them afterwards.
    fn squash(
        &self,
        ids: &[Oid],
        message: &str,
        autostash: bool,
    ) -> impl Future<Output = Result<Integration>> + Send;

    /// Runs the interactive rebase described by `items` onto `base`
    /// (`None` = rewrite from the root).
    fn rebase_interactive(
        &self,
        base: Option<&Oid>,
        items: &[TodoItem],
    ) -> impl Future<Output = Result<Integration>> + Send;
}

/// Reflog, HEAD and moving branches back (reset, undo).
pub trait ReflogOps {
    /// Most recent HEAD movements, newest first.
    fn reflog(&self, limit: usize) -> impl Future<Output = Result<Vec<ReflogEntry>>> + Send;

    /// Current HEAD commit, `None` before the first commit.
    fn head_oid(&self) -> impl Future<Output = Result<Option<Oid>>> + Send;

    /// `git reset --soft|--mixed|--hard <to>`: moves the current branch
    /// (or a detached HEAD) to `to`.
    fn reset(&self, to: &Oid, mode: ResetMode) -> impl Future<Output = Result<()>> + Send;

    /// Moves the current branch back to `to`.
    ///
    /// With `soft`, the undone commits' changes stay staged (undoing a
    /// commit gives the work back); otherwise `reset --keep` restores the
    /// files too but refuses to overwrite uncommitted changes.
    fn undo_to(&self, to: &Oid, soft: bool) -> impl Future<Output = Result<()>> + Send;
}

/// Branch and tag listings.
pub trait RefOps {
    /// Local and remote-tracking branches, with upstream ahead/behind counts.
    fn branches(&self) -> impl Future<Output = Result<Vec<Branch>>> + Send;

    fn tags(&self) -> impl Future<Output = Result<Vec<Tag>>> + Send;

    /// Commits reachable from HEAD that no remote-tracking branch contains:
    /// what a first push of an unpublished branch would upload. `0` before
    /// the first commit.
    fn unpushed_count(&self) -> impl Future<Output = Result<u32>> + Send;

    /// Commits reachable from HEAD that are on no remote and not reachable
    /// from `since`, newest first.
    fn unpushed(&self, since: Option<&Oid>) -> impl Future<Output = Result<Vec<Oid>>> + Send;

    /// Like [`Self::unpushed`], but only the newest `limit` commits (a
    /// branch with no remote at all would otherwise list its whole history).
    fn unpushed_recent(&self, limit: usize) -> impl Future<Output = Result<Vec<Oid>>> + Send;
}

/// Configured remotes.
pub trait RemoteOps {
    fn remotes(&self) -> impl Future<Output = Result<Vec<Remote>>> + Send;
}

/// Stashes.
pub trait StashOps {
    fn stashes(&self) -> impl Future<Output = Result<Vec<StashEntry>>> + Send;

    /// Saves local changes to a new stash entry.
    fn stash_push(&self, opts: &StashOptions) -> impl Future<Output = Result<()>> + Send;

    /// Applies a stash, keeping it in the list. `--index` restores what
    /// was staged as staged.
    fn stash_apply(&self, index: usize) -> impl Future<Output = Result<()>> + Send;

    /// Applies a stash and removes it when that succeeded.
    fn stash_pop(&self, index: usize) -> impl Future<Output = Result<()>> + Send;

    /// Deletes a stash entry. Irreversible from the UI's point of view.
    fn stash_drop(&self, index: usize) -> impl Future<Output = Result<()>> + Send;
}

/// Operation in progress (merge, rebase…).
pub trait StateOps {
    fn state(&self) -> impl Future<Output = Result<RepoState>> + Send;
}

/// Working tree status.
pub trait StatusOps {
    /// Working tree and index status plus HEAD/upstream info.
    fn status(&self) -> impl Future<Output = Result<StatusReport>> + Send;
}

/// Submodules.
pub trait SubmoduleOps {
    fn submodules(&self) -> impl Future<Output = Result<Vec<Submodule>>> + Send;

    /// `git submodule update --init --recursive`: checks out the recorded
    /// commits, initializing new submodules.
    fn update_submodules(&self) -> impl Future<Output = Result<()>> + Send;

    /// Re-reads submodule URLs from `.gitmodules`.
    fn sync_submodules(&self) -> impl Future<Output = Result<()>> + Send;
}

/// Fetch, pull and push.
pub trait SyncOps {
    /// `git fetch --prune` from one remote or all of them.
    fn fetch(
        &self,
        remote: Option<&str>,
        opts: RunOptions,
    ) -> impl Future<Output = Result<()>> + Send;

    fn pull(&self, mode: PullMode, opts: RunOptions) -> impl Future<Output = Result<()>> + Send;

    fn push(&self, push: &PushOptions, opts: RunOptions)
    -> impl Future<Output = Result<()>> + Send;
}

/// Tags.
pub trait TagOps {
    /// Creates a tag at `target` (default HEAD). With a message the tag is
    /// annotated, otherwise lightweight.
    fn create_tag(
        &self,
        name: &str,
        target: Option<&str>,
        message: Option<&str>,
    ) -> impl Future<Output = Result<()>> + Send;

    fn delete_tag(&self, name: &str) -> impl Future<Output = Result<()>> + Send;

    fn push_tag(&self, remote: &str, name: &str) -> impl Future<Output = Result<()>> + Send;
}

/// Linked worktrees.
/// Capturing and restoring state for undo / redo (see `ops::undo`).
pub trait UndoOps {
    /// Files a stash entry changes (tracked, staged and untracked parts):
    /// what applying or popping it touches.
    fn stash_paths(&self, index: usize) -> impl Future<Output = Result<Vec<PathBuf>>> + Send;
    /// The state of everything `scope` covers. Saves work tree files as
    /// blobs so they can be written back later.
    fn capture(&self, scope: &Scope) -> impl Future<Output = Result<Capture>> + Send;
    /// Puts back what `to` captured (HEAD moved with `mode`), after
    /// checking the repository still looks like `expected` (when given).
    fn restore(
        &self,
        scope: &Scope,
        expected: Option<&Capture>,
        to: &Capture,
        mode: HeadMode,
    ) -> impl Future<Output = Result<()>> + Send;
    /// How many commits that leave `branch` when it moves from `from` to
    /// `to` (`None`: unborn) are already on its upstream — undoing them
    /// locally does not take them off the remote. 0 without an upstream.
    /// Keeps the saved work tree copies `blobs` from `git gc` by pointing
    /// [`crate::UNDO_REF`] at a tree of them (deleted when there are
    /// none). Ids that no longer exist are left out.
    fn pin_saved_files(&self, blobs: &[String]) -> impl Future<Output = Result<()>> + Send;
    fn published_leaving(
        &self,
        branch: &str,
        from: &str,
        to: Option<&str>,
    ) -> impl Future<Output = Result<usize>> + Send;
}

pub trait WorktreeOps {
    fn worktrees(&self) -> impl Future<Output = Result<Vec<Worktree>>> + Send;

    /// Adds a working tree at `path` for `branch`; with `new_branch`, the
    /// branch is created from HEAD.
    fn add_worktree(
        &self,
        path: &Path,
        branch: &str,
        new_branch: bool,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Removes a linked working tree. Without `force`, git refuses when it
    /// has uncommitted changes.
    fn remove_worktree(&self, path: &Path, force: bool) -> impl Future<Output = Result<()>> + Send;

    fn prune_worktrees(&self) -> impl Future<Output = Result<()>> + Send;
}
