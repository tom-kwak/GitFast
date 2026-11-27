//! Dialogs: input dialogs (new branch, tag, stash…) and the shared
//! confirmation used before every delete or change.
//!
//! # Confirming an action (reuse this for every new feature)
//!
//! ```ignore
//! self.confirm(
//!     Confirm::change("pull", "Pull", vec!["…what will happen…".into()], "Pull", purpose)
//! )
//! ```
//!
//! - [`Confirm::delete`] for anything that removes data: red, Enter does not
//!   confirm, never skippable.
//! - [`Confirm::change`] for anything that changes the working tree, HEAD or
//!   a remote: yellow, Enter confirms, and the user may tick "don't ask
//!   again" for that key (stored in settings, `:confirm reset` restores).
//!
//! The action itself is a [`Purpose`] variant, run by [`App::run_purpose`]
//! once confirmed (or immediately when the user chose not to be asked).

use std::path::PathBuf;

use gf_git::{PullMode, PushOptions};
use iced::Task;
use iced::widget::operation;

use super::ops::{Changes, Record};
use super::outputs::{names, stash_options};
use super::{App, Message, OpKind};
use crate::features::command_log::NoteKind;
use crate::features::dialog::{Dialog, FIRST_FIELD_ID, Outcome, Severity};
use crate::features::diff_view::Action;
use crate::repo_tab::PendingTags;
use crate::undo;

/// What a dialog is for; carried through the dialog and acted on at submit.
#[derive(Debug, Clone)]
pub enum Purpose {
    Discard {
        tracked: Vec<PathBuf>,
        untracked: Vec<PathBuf>,
        /// The dialog promised ⌘Z: refuse to run without a saved copy.
        saved: bool,
    },
    ApplyPatches {
        patches: Vec<String>,
        action: Action,
        description: String,
    },
    NewBranch {
        from: Option<String>,
        unborn: bool,
    },
    RenameBranch {
        old: String,
    },
    DeleteBranches(super::branch_delete::BranchDeletion),
    NewTag {
        at: Option<String>,
    },
    DeleteTag(String),
    Stash,
    StashDrop(usize),
    Publish {
        branch: String,
        /// Tags the remote does not have, sent along to that remote.
        tags: Option<PendingTags>,
    },
    ForcePush {
        branch: Option<String>,
        to: Option<super::PushTo>,
    },
    PullStrategy(gf_core::model::RepoId),
    CheckoutDetached(String),
    /// Go to the tab of the worktree that has a branch checked out.
    OpenWorktreeTab(PathBuf),
    /// A linked tree asked for its base: look at it detached (`rev`), or
    /// with the check ticked, go to the main tree's tab.
    LinkedBase {
        rev: String,
        main: Option<PathBuf>,
    },
    Checkout {
        target: String,
        name: String,
        remote: bool,
        /// Stash the changes first, as left on this branch ("stash, then
        /// switch"); `None` takes them along.
        leave_from: Option<String>,
        /// Changed files that keep them from coming along (`l` lists them).
        blocked: Vec<PathBuf>,
    },
    StashApply(usize),
    StashPop(usize),
    Pull {
        repo: gf_core::model::RepoId,
        mode: PullMode,
    },
    Push {
        repo: gf_core::model::RepoId,
        branch: String,
        /// Set for a branch that is not checked out.
        to: Option<super::PushTo>,
        /// Tags the remote does not have, sent along (checked-out branch).
        tags: Option<PendingTags>,
        allow_configured_rewrites: bool,
    },
    PushTag {
        remote: String,
        name: String,
    },
    Amend {
        message: String,
    },
    Merge {
        branch: String,
    },
    RebaseOnto {
        onto: String,
    },
    /// Oldest first.
    CherryPick(Vec<gf_core::model::Oid>),
    /// Newest first.
    Revert(Vec<gf_core::model::Oid>),
    RebaseInteractive {
        repo: gf_core::model::RepoId,
        base: gf_core::model::Oid,
        items: Vec<gf_git::TodoItem>,
    },
    BisectStart {
        good: gf_core::model::Oid,
    },
    /// Move the current branch to `to`; the mode comes from the dialog's
    /// choice (see [`RESET_MODES`]).
    Reset {
        to: gf_core::model::Oid,
        mode: gf_git::ResetMode,
    },
    BisectReset,
    /// Meld `ids` (newest first) into one commit. Values: summary, details;
    /// check 0 (when offered): stash uncommitted changes around it.
    Squash {
        repo: gf_core::model::RepoId,
        ids: Vec<gf_core::model::Oid>,
    },
    /// A failure report was dismissed; nothing to run.
    Failure,
    /// Fields: server, user name, token (see `App::login_dialog`); then
    /// run the push/pull/fetch the server refused, if any.
    LogIn(Option<Box<super::settings::Retry>>),
    /// Fetch all remotes of the active tab (a retry after logging in).
    Fetch,
    /// Run a git command typed at the prompt (a retry after logging in).
    RunGit(Vec<String>),
    /// Fields: URL, folder. Clone it there, then open it.
    Clone,
    /// Save the hosting service picked for `host` (the option chosen).
    SetService {
        host: String,
        service: Option<gf_hosting::ProviderKind>,
    },
    /// Forget the login (`settings.accounts`) in git's credential helper.
    LogOut(gf_hosting::Account),
    /// Set git's global `credential.helper` so logins can be kept.
    UseCredentialHelper(String),
    /// Field: one of the TOOLS settings (Settings → TOOLS).
    SetTool(super::settings::Tool),
    SkipOperation,
    AbortOperation,
    /// Undo (or redo) the next step; the check box stashes first.
    Undo {
        redo: bool,
        /// "Undo anyway": overwrite what changed since the step.
        force: bool,
    },
    SaveConflict {
        path: PathBuf,
        original: std::sync::Arc<gf_git::ConflictDocument>,
        result: String,
    },
    ReplaceConflictChoice(usize, gf_git::conflict::Choice),
    DiscardConflictDraft(PathBuf),
    TakeSide(PathBuf, gf_git::Side),
    /// Load the history down to a far commit, then show it.
    RevealCommit {
        oid: gf_core::model::Oid,
        at: super::reveal::Located,
    },
    /// Reveal a search result after confirming the longer load.
    RevealSearchCommit {
        id: gf_core::model::RepoId,
        generation: u64,
        oid: gf_core::model::Oid,
        rows: usize,
    },
    MarkResolved(PathBuf),
    /// Fields: branch, folder (empty: `root/<branch>`). Choice: start a
    /// new branch at HEAD or `base`. Check: open it in a tab. A branch in
    /// `existing` (local, or a remote's) is checked out, not created; one
    /// in `checked_out` is refused (git allows a branch in one tree only).
    AddWorktree {
        root: PathBuf,
        base: Option<String>,
        existing: Vec<String>,
        checked_out: Vec<String>,
    },
    /// `force` drops the uncommitted changes the dialog told about.
    RemoveWorktree {
        path: PathBuf,
        force: bool,
    },
    LockWorktree(PathBuf, bool),
    PruneWorktrees,
    /// Merge `branch` into its base, checked out in the tree at `into`
    /// (`real`: resolved). Checks: always a merge commit; then remove
    /// `clean` (offered only when there is one).
    MergeIntoBase {
        branch: String,
        into: PathBuf,
        real: Option<PathBuf>,
        clean: Option<super::worktree_ops::CleanAfterMerge>,
    },
    /// Bring the base's new commits into a tree's branch. Check: rebase
    /// instead of merge.
    UpdateFromBase(super::worktree_ops::UpdateFromBase),
    /// Field: the local branch `branch` goes back into; empty forgets it
    /// (it is guessed again). `local`: the branches that can be.
    SetBranchBase {
        branch: String,
        local: Vec<String>,
    },
    /// Trees merged into `base` and their branches. Check: delete the
    /// branches too.
    CleanupWorktrees {
        base: String,
        trees: Vec<(PathBuf, Option<String>)>,
    },
    UpdateSubmodules,
    TrackLfs,
    UntrackLfs(String),
    LfsPull,
}

/// A confirmation request; build with [`Confirm::delete`] or
/// [`Confirm::change`] and pass to [`App::confirm`].
#[derive(Debug, Clone)]
pub struct Confirm {
    severity: Severity,
    /// Settings key for "don't ask again" (change confirmations only).
    key: Option<&'static str>,
    title: String,
    body: Vec<String>,
    label: String,
    review: Vec<String>,
    checks: Vec<(String, bool)>,
    then: Purpose,
    /// ⌘Z can undo it.
    undoable: bool,
    /// Nothing worth asking about this time: run without the dialog.
    skip: bool,
}

impl Confirm {
    /// Removes data: red, click to confirm, always asked.
    pub fn delete(
        title: impl Into<String>,
        body: Vec<String>,
        label: impl Into<String>,
        then: Purpose,
    ) -> Self {
        Self {
            severity: Severity::Delete,
            key: None,
            title: title.into(),
            body,
            label: label.into(),
            review: Vec::new(),
            checks: Vec::new(),
            then,
            undoable: false,
            skip: false,
        }
    }

    /// Changes the working tree, HEAD or a remote: yellow, Enter confirms,
    /// skippable per `key`.
    pub fn change(
        key: &'static str,
        title: impl Into<String>,
        body: Vec<String>,
        label: impl Into<String>,
        then: Purpose,
    ) -> Self {
        Self {
            severity: Severity::Change,
            key: Some(key),
            title: title.into(),
            body,
            label: label.into(),
            review: Vec::new(),
            checks: Vec::new(),
            then,
            undoable: false,
            skip: false,
        }
    }

    /// A change that must always be asked (no "don't ask again").
    pub fn always(mut self) -> Self {
        self.key = None;
        self
    }

    /// ⌘Z can undo it: the dialog says so instead of "cannot be undone".
    pub fn undoable(mut self) -> Self {
        self.undoable = true;
        self
    }

    /// Runs without asking when `safe` (a change with nothing to lose this
    /// time, e.g. switching branch with no changes). Still goes through
    /// [`App::confirm`] so every change has one way in.
    pub fn skip_if(mut self, safe: bool) -> Self {
        self.skip = safe;
        self
    }

    pub fn review(mut self, entries: Vec<String>) -> Self {
        self.review = entries;
        self
    }

    pub fn check(mut self, label: impl Into<String>, value: bool) -> Self {
        self.checks.push((label.into(), value));
        self
    }
}

impl App {
    /// Asks before running `c.then` — the one way to confirm a delete or
    /// change. A change runs right away when the user ticked "don't ask
    /// again" for it before, or the caller said it is safe this time
    /// ([`Confirm::skip_if`]). A delete is always asked.
    pub(super) fn confirm(&mut self, c: Confirm) -> Task<Message> {
        let skipped = c
            .key
            .is_some_and(|key| self.settings.confirm.is_skipped(key));
        if c.severity == Severity::Change && (c.skip || skipped) {
            let checks = c.checks.iter().map(|(_, v)| *v).collect();
            return self.run_purpose(c.then, Vec::new(), checks);
        }
        let mut dialog = Dialog::confirm(c.title, c.body, c.label, c.then)
            .severity(c.severity)
            .review(c.review);
        for (label, value) in c.checks {
            dialog = dialog.check(label, value);
        }
        if let Some(key) = c.key {
            dialog = dialog.remember_as(key);
        }
        if c.undoable {
            dialog = dialog.undoable();
        }
        self.open_dialog(dialog)
    }

    pub(super) fn open_dialog(&mut self, dialog: Dialog<Purpose>) -> Task<Message> {
        let focus = dialog.has_fields();
        self.dialog = Some(dialog);
        if focus {
            operation::focus(FIRST_FIELD_ID)
        } else {
            Task::none()
        }
    }

    pub(super) fn new_branch_dialog(
        &mut self,
        from: Option<String>,
        branch_here: bool,
    ) -> Task<Message> {
        let unborn = self
            .active_tab()
            .and_then(|tab| tab.snapshot.as_ref())
            .is_some_and(|s| s.status.head.oid.is_none() && s.status.head.branch.is_some());
        if unborn {
            return self.open_dialog(
                Dialog::confirm(
                    "New branch",
                    vec![
                        "There are no commits yet. The first commit will go on this branch.".into(),
                    ],
                    "Create",
                    Purpose::NewBranch { from: None, unborn },
                )
                .field("name", "", "feature/my-change"),
            );
        }

        let suggested = from
            .as_deref()
            .and_then(|f| f.strip_prefix("refs/remotes/"))
            .and_then(|f| f.split_once('/'))
            .map(|(_, b)| b.to_owned())
            .unwrap_or_default();
        let start = self
            .active_tab()
            .and_then(|tab| tab.snapshot.as_ref())
            .and_then(|s| s.status.head.branch.as_deref());
        let head = start.map_or_else(|| "HEAD".to_owned(), |name| format!("{name} (HEAD)"));
        let other = from
            .as_deref()
            .filter(|f| start.is_none_or(|name| f.strip_prefix("refs/heads/") != Some(name)))
            .map(|f| short_ref(f).to_owned());
        let body = if other.is_none() {
            vec![format!("Start at {head}.")]
        } else {
            vec![]
        };
        let mut dialog = Dialog::confirm(
            "New branch",
            body,
            "Create",
            Purpose::NewBranch { from, unborn },
        )
        .field("name", suggested, "feature/my-change")
        .check("switch to the new branch", true);
        if let Some(other) = other {
            dialog = dialog.choice(
                "start at",
                vec![(head, false), (other, false)],
                usize::from(branch_here),
            );
        }
        self.open_dialog(dialog)
    }

    /// Asks for a new worktree; `branch`: the branch field filled in
    /// (picked from the branch menu).
    pub(super) fn new_worktree_dialog(&mut self, branch: Option<String>) -> Task<Message> {
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        let root = self.settings.worktrees.root.clone().unwrap_or_else(|| {
            let cwd = tab.repo.cwd();
            cwd.parent()
                .unwrap_or(cwd)
                .join(format!("{}-worktrees", tab.name()))
        });
        let head = tab.head_label().unwrap_or_else(|| "HEAD".into());
        let base = tab
            .worktrees
            .base()
            .filter(|b| tab.head_label().as_deref() != Some(*b))
            .map(str::to_owned);
        // Branches that exist: local ones, and remote ones by their short
        // name (git creates the local branch tracking it).
        let existing = tab
            .snapshot
            .as_ref()
            .map(|snap| {
                snap.branches
                    .iter()
                    .map(|b| {
                        let short = b.name.short();
                        match b.name.kind() {
                            gf_core::model::RefKind::RemoteBranch => short
                                .split_once('/')
                                .map_or(short, |(_, name)| name)
                                .to_owned(),
                            _ => short.to_owned(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let checked_out: Vec<String> = tab
            .worktrees
            .cards()
            .iter()
            .filter_map(|c| c.tree.branch.clone())
            .collect();
        let suggestions = tab
            .snapshot
            .as_ref()
            .map(|snap| branch_suggestions(&snap.branches, &checked_out))
            .unwrap_or_default();
        let body = branch.as_ref().map_or_else(
            || "Check a branch out in its own folder.".to_owned(),
            |b| format!("Check {b} out in its own folder."),
        );
        // An existing branch was picked: where a new one starts is moot.
        let picked = branch.is_some();
        let mut dialog = Dialog::confirm(
            "New worktree",
            vec![body],
            "Add",
            Purpose::AddWorktree {
                root: root.clone(),
                base: base.clone(),
                existing,
                checked_out,
            },
        )
        .field(
            "branch",
            branch.clone().unwrap_or_default(),
            "new or existing branch",
        )
        .suggest(suggestions, "new branch")
        .field(
            "folder",
            "",
            // The folder it gets, once the branch is known.
            branch.as_deref().map_or_else(
                || format!("{}/<branch>", crate::widgets::format::tilde(&root)),
                |b| crate::widgets::format::tilde(&root.join(worktree_folder_name(b))),
            ),
        )
        .check("open it in a tab", true);
        if let Some(base) = base.filter(|_| !picked) {
            // From a branch work goes into (develop), start there; from a
            // feature branch, at the base.
            let at_head = ["main", "master", "develop", "development", "dev", "trunk"]
                .contains(&head.as_str());
            dialog = dialog.choice(
                "a new branch starts at",
                vec![(format!("{head} (HEAD)"), false), (base, false)],
                usize::from(!at_head),
            );
        }
        self.open_dialog(dialog)
    }

    pub(super) fn new_tag_dialog(&mut self, at: Option<String>) -> Task<Message> {
        let target = at
            .as_deref()
            .map_or_else(|| "HEAD".to_owned(), |a| a.chars().take(7).collect());
        self.open_dialog(
            Dialog::confirm(
                "New tag",
                vec![format!("Tag {target}.")],
                "Create",
                Purpose::NewTag { at },
            )
            .field("name", "", "v1.0.0")
            .field(
                "message (optional — makes an annotated tag)",
                "",
                "Release notes…",
            ),
        )
    }

    pub(super) fn stash_dialog(&mut self) -> Task<Message> {
        if self
            .active_tab()
            .and_then(|tab| tab.snapshot.as_ref())
            .is_some_and(|s| !s.has_changes)
        {
            self.flash(NoteKind::Info, "nothing to stash");
            return Task::none();
        }
        self.open_dialog(
            Dialog::confirm(
                "Stash changes",
                vec!["Put your local changes aside and get a clean working tree.".into()],
                "Stash",
                Purpose::Stash,
            )
            .field("message (optional)", "", "what you were doing")
            .check("include untracked files", true)
            .check("keep staged changes in the index", false),
        )
    }

    pub(super) fn on_dialog_outcome(&mut self, outcome: Outcome<Purpose>) -> Task<Message> {
        let (purpose, values, checks, dont_ask_again) = match outcome {
            Outcome::Submitted {
                purpose,
                values,
                checks,
                choice,
                dont_ask_again,
            } => {
                // Map the selected option to its operation argument.
                let purpose = match purpose {
                    // Option 0 starts the branch at HEAD, 1 at the base.
                    Purpose::AddWorktree {
                        root,
                        base,
                        existing,
                        checked_out,
                    } => Purpose::AddWorktree {
                        root,
                        base: base.filter(|_| choice == Some(1)),
                        existing,
                        checked_out,
                    },
                    Purpose::NewBranch { from, unborn } => Purpose::NewBranch {
                        from: from.filter(|_| choice == Some(1)),
                        unborn,
                    },
                    Purpose::Reset { to, mode } => Purpose::Reset {
                        to,
                        mode: choice
                            .and_then(|i| RESET_MODES.get(i))
                            .map_or(mode, |(m, _, _)| *m),
                    },
                    // Option 0 leaves the changes behind, 1 carries them.
                    Purpose::Checkout {
                        target,
                        name,
                        remote,
                        leave_from,
                        blocked,
                    } => Purpose::Checkout {
                        target,
                        name,
                        remote,
                        leave_from: leave_from.filter(|_| choice != Some(1)),
                        blocked,
                    },
                    Purpose::SetService { host, .. } => Purpose::SetService {
                        host,
                        service: choice.and_then(|i| gf_hosting::ProviderKind::ALL.get(i).copied()),
                    },
                    // The service picked beside the server is kept for it.
                    Purpose::LogIn(retry) => {
                        let picked =
                            choice.and_then(|i| gf_hosting::ProviderKind::ALL.get(i).copied());
                        if let Some(server) = values.first() {
                            self.remember_service(server, picked);
                        }
                        Purpose::LogIn(retry)
                    }
                    other => other,
                };
                (purpose, values, checks, dont_ask_again)
            }
            Outcome::Cancelled { purpose } => {
                self.dialog = None;
                // The commit box waits for the amend to run; release it.
                if matches!(purpose, Purpose::Amend { .. })
                    && let Some(tab) = self.active_tab_mut()
                {
                    tab.commit.commit_failed();
                }
                // Undoing several steps stops at the one not confirmed.
                if matches!(purpose, Purpose::Undo { .. })
                    && let Some(tab) = self.active_tab_mut()
                {
                    tab.undo_queue = None;
                }
                return self.restore_focus();
            }
        };

        // Validate before closing so the user can fix the input in place.
        if let Some(error) = validate(&purpose, &values) {
            if let Some(d) = self.dialog.as_mut() {
                d.error = Some(error);
            }
            return Task::none();
        }
        self.dialog = None;
        if let Some(key) = dont_ask_again {
            self.settings.confirm.skip(&key);
            self.save_settings();
            self.flash(
                NoteKind::Info,
                format!("won't ask before {key} again — `:confirm reset` to undo"),
            );
        }
        self.run_purpose(purpose, values, checks)
    }

    /// Runs a confirmed action. `values`/`checks` are the dialog's fields
    /// and check boxes, in order.
    pub(super) fn run_purpose(
        &mut self,
        purpose: Purpose,
        values: Vec<String>,
        checks: Vec<bool>,
    ) -> Task<Message> {
        let value = |i: usize| values.get(i).cloned().unwrap_or_default();
        let check = |i: usize| checks.get(i).copied().unwrap_or(false);
        match purpose {
            Purpose::Discard {
                tracked,
                untracked,
                saved,
            } => {
                let subs = self.submodule_paths(&tracked);
                let scope = gf_git::Scope {
                    files: tracked
                        .iter()
                        .chain(&untracked)
                        .filter(|p| !subs.contains(p))
                        .cloned()
                        .collect(),
                    submodules: subs,
                    ..gf_git::Scope::default()
                };
                self.op_recorded(
                    format!(
                        "discard {}",
                        names(&[tracked.clone(), untracked.clone()].concat())
                    ),
                    OpKind::Generic,
                    Some(if saved {
                        Record::new(scope).required()
                    } else {
                        Record::new(scope)
                    }),
                    move |r| async move { r.discard(&tracked, &untracked).await },
                )
            }
            Purpose::ApplyPatches {
                patches,
                action,
                description,
            } => self.apply_patches(patches, action, description),
            Purpose::NewBranch { from, unborn } => {
                let name = value(0);
                let checkout = unborn || check(0);
                let label = if unborn {
                    format!("next commit goes on {name}")
                } else {
                    format!("create branch {name}")
                };
                let target = format!("refs/heads/{name}");
                let record = Record::new(undo::branches(&[&name], checkout));
                let create = move |r: gf_git::Repository| async move {
                    r.create_branch(&name, from.as_deref(), checkout).await
                };
                if checkout {
                    self.checkout_op(target, label, record, create)
                } else {
                    self.op_recorded(label, OpKind::Generic, Some(record), create)
                }
            }
            Purpose::RenameBranch { old } => {
                let new = value(0);
                self.op_recorded(
                    format!("rename {old} → {new}"),
                    OpKind::Generic,
                    Some(Record::new(undo::branches(&[&old, &new], true))),
                    move |r| async move { r.rename_branch(&old, &new).await },
                )
            }
            Purpose::DeleteBranches(selection) => self.run_delete_branches(selection, check(0)),
            Purpose::NewTag { at } => {
                let name = value(0);
                let message = value(1);
                self.op_recorded(
                    format!("create tag {name}"),
                    OpKind::Generic,
                    Some(Record::new(undo::tag(&name))),
                    move |r| async move {
                        let msg = (!message.is_empty()).then_some(message.as_str());
                        r.create_tag(&name, at.as_deref(), msg).await
                    },
                )
            }
            Purpose::DeleteTag(name) => self.op_recorded(
                format!("delete tag {name}"),
                OpKind::Generic,
                Some(Record::new(undo::tag(&name)).required()),
                move |r| async move { r.delete_tag(&name).await },
            ),
            Purpose::Stash => {
                let opts = stash_options(value(0), check(0), check(1));
                let record = Record::new(undo::stash(Vec::new())).with_changes(Changes::All);
                self.op_recorded(
                    "stash changes",
                    OpKind::Generic,
                    Some(record),
                    move |r| async move { r.stash_push(&opts).await },
                )
            }
            Purpose::StashDrop(i) => self.op_recorded(
                format!("drop stash@{{{i}}}"),
                OpKind::Generic,
                Some(
                    Record::new(gf_git::Scope {
                        stash: true,
                        ..gf_git::Scope::default()
                    })
                    .required(),
                ),
                move |r| async move { r.stash_drop(i).await },
            ),
            Purpose::Publish { branch, tags } => {
                // The tags go along when they are for the remote chosen.
                let tags = tags.filter(|t| t.remote == value(0));
                let opts = PushOptions {
                    remote: Some(value(0)),
                    branch: Some(branch.clone()),
                    set_upstream: true,
                    ..PushOptions::default()
                };
                let label = format!("publish {branch}");
                let to = opts.remote.clone().map(|remote| super::PushTo {
                    remote,
                    refspec: format!("{branch}:{branch}"),
                });
                self.sync_op(
                    label,
                    OpKind::Push {
                        branch: Some(branch),
                        to,
                        offer_force: true,
                        tags: None,
                    },
                    move |r, run| async move {
                        r.push(&opts, run).await?;
                        push_tags(&r, tags).await
                    },
                )
            }
            Purpose::ForcePush { branch, to } => {
                let opts = PushOptions {
                    force_with_lease: true,
                    ..push_options(to.as_ref())
                };
                self.sync_op(
                    "force push (with lease)",
                    OpKind::Push {
                        branch,
                        to,
                        offer_force: false,
                        tags: None,
                    },
                    move |r, run| async move { r.push(&opts, run).await },
                )
            }
            Purpose::PullStrategy(repo) => self.pull(
                repo,
                if check(0) {
                    PullMode::Rebase
                } else {
                    PullMode::Merge
                },
            ),
            Purpose::OpenWorktreeTab(path) => self.update(Message::Open(path)),
            Purpose::LinkedBase {
                main: Some(main), ..
            } if check(0) => self.update(Message::Open(main)),
            Purpose::LinkedBase { rev, .. } => {
                self.run_purpose(Purpose::CheckoutDetached(rev), Vec::new(), Vec::new())
            }
            Purpose::CheckoutDetached(rev) => {
                let short: String = rev.chars().take(7).collect();
                self.checkout_op(
                    short.clone(),
                    format!("check out {short}"),
                    Record::head(gf_git::HeadMode::Keep),
                    move |r| async move { r.checkout_detached(&rev).await },
                )
            }
            Purpose::Checkout {
                target,
                name,
                remote,
                leave_from,
                ..
            } => {
                let mut label = if remote {
                    format!("check out {name} as a tracking branch")
                } else {
                    format!("switch to {name}")
                };
                if let Some(from) = &leave_from {
                    label = format!("leave changes on {from}, {label}");
                }
                // Checking out a remote branch also creates the local one.
                let local = remote
                    .then(|| name.split_once('/').map(|(_, b)| b.to_owned()))
                    .flatten();
                let mut scope = match &local {
                    Some(b) => undo::branches(&[b], true),
                    None => gf_git::Scope::head(),
                };
                match leave_from {
                    // One step: ⌘Z switches back and pops the changes.
                    Some(from) => {
                        scope.stash = true;
                        self.checkout_op(
                            target,
                            label,
                            Record::new(scope).with_changes(Changes::All),
                            move |r| async move { r.stash_and_switch(&from, &name, remote).await },
                        )
                    }
                    None => {
                        self.checkout_op(target, label, Record::new(scope), move |r| async move {
                            r.switch_branch(&name, remote).await
                        })
                    }
                }
            }
            Purpose::StashApply(i) => self.op_recorded(
                format!("apply stash@{{{i}}}"),
                OpKind::Generic,
                Some(
                    Record::new(undo::stash(Vec::new()))
                        .with_stash_entry(i)
                        .with_changes(Changes::All),
                ),
                move |r| async move { r.stash_apply(i).await },
            ),
            Purpose::StashPop(i) => self.op_recorded(
                format!("pop stash@{{{i}}}"),
                OpKind::Generic,
                Some(
                    Record::new(undo::stash(Vec::new()))
                        .with_stash_entry(i)
                        .with_changes(Changes::All),
                ),
                move |r| async move { r.stash_pop(i).await },
            ),
            Purpose::Pull { repo, mode } => self.pull(repo, mode),
            Purpose::Push {
                repo,
                branch,
                to,
                tags,
                allow_configured_rewrites,
            } => {
                if self.active_tab().is_none_or(|tab| tab.repo.id() != repo) {
                    return self.restore_focus();
                }
                let guard = to.is_none() && !allow_configured_rewrites;
                let opts = push_options(to.as_ref());
                self.sync_op(
                    format!("push {branch}"),
                    OpKind::Push {
                        branch: Some(branch.clone()),
                        to,
                        offer_force: true,
                        tags: tags.clone(),
                    },
                    move |r, run| async move {
                        if guard {
                            r.guard_default_push(&branch).await?;
                        }
                        r.push(&opts, run).await?;
                        push_tags(&r, tags).await
                    },
                )
            }
            Purpose::PushTag { remote, name } => self.op(
                format!("push tag {name}"),
                OpKind::Remote(super::RemoteOp::PushTag {
                    remote: remote.clone(),
                    name: name.clone(),
                }),
                // Recorded as on the remote, so push stops counting it.
                move |r| async move { r.push_tags(&remote, &[name]).await },
            ),
            Purpose::Merge { branch } => self.run_merge(branch, check(0)),
            Purpose::RebaseOnto { onto } => self.run_rebase_onto(onto),
            Purpose::CherryPick(oids) => self.run_cherry_pick(oids),
            Purpose::Revert(oids) => self.run_revert(oids),
            Purpose::RebaseInteractive { repo, base, items } => {
                self.run_rebase_interactive(repo, base, items)
            }
            Purpose::BisectStart { good } => self
                .bisect_step(move |r| async move { r.bisect_start("HEAD", good.as_str()).await }),
            Purpose::Failure => self.restore_focus(),
            Purpose::LogIn(retry) => self.log_in(&values, retry),
            Purpose::Fetch => self.on_toolbar(crate::features::toolbar::Message::Fetch),
            Purpose::RunGit(args) => self.run_git(args),
            Purpose::Clone => self.run_clone(&value(0), &value(1)),
            Purpose::LogOut(account) => self.log_out(account),
            Purpose::SetService { host, service } => self.set_service(&host, service),
            Purpose::UseCredentialHelper(helper) => self.use_credential_helper(helper),
            Purpose::SetTool(tool) => self.set_tool(tool, &value(0)),
            Purpose::Squash { repo, ids } => {
                let message = super::advanced::commit_message(&value(0), &value(1));
                self.run_squash(repo, ids, message, check(0))
            }
            Purpose::BisectReset => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.bisect = None;
                }
                self.op("end bisect", OpKind::Generic, |r| async move {
                    r.bisect_reset().await
                })
            }
            Purpose::SkipOperation => {
                self.integrate_op(
                    "skip commit",
                    false,
                    |r| async move { r.skip_operation().await },
                )
            }
            Purpose::AbortOperation => {
                // Aborting puts everything back: nothing left to record.
                if let Some(tab) = self.active_tab_mut() {
                    tab.undo.set_pending(None, &tab.repo.handle().git_dir);
                }
                self.op("abort", OpKind::Generic, |r| async move {
                    r.abort_operation().await
                })
            }
            Purpose::Undo { redo, force } => self.run_undo(redo, check(0), force),
            Purpose::Reset { to, mode } => {
                let label = format!(
                    "reset {} to {}",
                    match mode {
                        gf_git::ResetMode::Soft => "--soft",
                        gf_git::ResetMode::Mixed => "--mixed",
                        gf_git::ResetMode::Hard => "--hard",
                    },
                    to.short()
                );
                // Undo moves the branch back the same way it moved; a hard
                // reset also saves the uncommitted work it throws away.
                let record = match mode {
                    gf_git::ResetMode::Soft => Record::head(gf_git::HeadMode::Soft),
                    gf_git::ResetMode::Mixed => Record::head(gf_git::HeadMode::Mixed),
                    // The dialog promised ⌘Z: the changes it throws away are
                    // read fresh and saved, or the reset does not run.
                    gf_git::ResetMode::Hard => Record::new(gf_git::Scope::head())
                        .with_changes(Changes::Tracked)
                        .required(),
                };
                self.op_recorded(label, OpKind::Generic, Some(record), move |r| async move {
                    r.reset(&to, mode).await
                })
            }
            Purpose::AddWorktree {
                root,
                base,
                existing,
                ..
            } => {
                let branch = value(0);
                let folder = match value(1) {
                    f if f.is_empty() => root.join(worktree_folder_name(&branch)),
                    f => crate::widgets::format::expand_tilde(&f),
                };
                let new_branch = !existing.contains(&branch);
                let open = check(0);
                let path = folder.clone();
                let include = self.settings.worktrees.copy_included;
                self.op(
                    format!("add worktree {branch}"),
                    OpKind::AddWorktree {
                        path,
                        open,
                        include,
                    },
                    move |r| async move {
                        r.add_worktree(&folder, &branch, new_branch, base.as_deref())
                            .await
                    },
                )
            }
            Purpose::LockWorktree(path, locked) => {
                let verb = if locked { "lock" } else { "unlock" };
                self.op(
                    format!("{verb} worktree {}", worktree_label(&path)),
                    OpKind::Generic,
                    move |r| async move { r.lock_worktree(&path, locked).await },
                )
            }
            Purpose::MergeIntoBase {
                branch,
                into,
                real,
                clean,
            } => {
                let clean = clean.filter(|_| check(1));
                self.merge_in_tree(branch, into, real.as_deref(), check(0), clean)
            }
            Purpose::UpdateFromBase(u) => {
                let what = if check(0) {
                    super::worktree_ops::Integrate::Rebase { onto: u.base }
                } else {
                    super::worktree_ops::Integrate::Merge {
                        branch: u.base,
                        no_ff: false,
                        clean: None,
                    }
                };
                self.integrate_in_tree(u.tree, u.real.as_deref(), what)
            }
            Purpose::SetBranchBase { branch, .. } => {
                let base = Some(value(0)).filter(|b| !b.is_empty());
                let label = match &base {
                    Some(base) => format!("base of {branch}: {base}"),
                    None => format!("forget the base of {branch}"),
                };
                self.op(label, OpKind::Generic, move |r| async move {
                    r.set_branch_base(&branch, base.as_deref()).await
                })
            }
            Purpose::PruneWorktrees => {
                self.op("prune worktrees", OpKind::Generic, |r| async move {
                    r.prune_worktrees().await
                })
            }
            Purpose::CleanupWorktrees { base, trees } => {
                let delete_branches = check(0);
                let paths = trees.iter().map(|(p, _)| p.clone()).collect();
                self.op(
                    format!("remove {} merged worktree(s)", trees.len()),
                    OpKind::RemoveWorktrees(paths),
                    move |r| async move { cleanup_worktrees(&r, &base, &trees, delete_branches).await },
                )
            }
            Purpose::RemoveWorktree { path, force } => self.op(
                format!("remove worktree {}", worktree_label(&path)),
                OpKind::RemoveWorktrees(vec![path.clone()]),
                move |r| async move { r.remove_worktree(&path, force).await },
            ),
            Purpose::UpdateSubmodules => {
                // Undo checks the previous commits out again in submodules
                // that were already there; newly cloned ones stay.
                let subs = self
                    .active_tab()
                    .and_then(|t| t.snapshot.as_ref())
                    .map(|s| s.submodules.iter().map(|m| m.path.clone()).collect())
                    .unwrap_or_default();
                self.op_recorded(
                    "update submodules",
                    OpKind::Remote(super::RemoteOp::UpdateSubmodules),
                    Some(Record::new(undo::submodules(subs))),
                    |r| async move { r.update_submodules().await },
                )
            }
            Purpose::TrackLfs => {
                let pattern = value(0);
                self.op_recorded(
                    format!("lfs track {pattern}"),
                    OpKind::Generic,
                    Some(Record::new(undo::file_and_index(".gitattributes".into()))),
                    move |r| async move { r.lfs_track(&pattern).await },
                )
            }
            Purpose::UntrackLfs(pattern) => self.op_recorded(
                format!("lfs untrack {pattern}"),
                OpKind::Generic,
                Some(Record::new(undo::file_and_index(".gitattributes".into()))),
                move |r| async move { r.lfs_untrack(&pattern).await },
            ),
            Purpose::LfsPull => self.op(
                "lfs pull",
                OpKind::Remote(super::RemoteOp::LfsPull),
                |r| async move { r.lfs_pull().await },
            ),
            Purpose::SaveConflict {
                path,
                original,
                result,
            } => self.run_resolve(path, original, result),
            Purpose::ReplaceConflictChoice(block, choice) => self.on_conflict_message(
                crate::features::conflict_view::Message::ReplaceChoice(block, choice),
            ),
            Purpose::DiscardConflictDraft(path) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.conflict = None;
                    tab.conflict_drafts.remove(&path);
                }
                self.open_conflict(path)
            }
            Purpose::RevealCommit { oid, at } => self.show_located(oid, at),
            Purpose::RevealSearchCommit {
                id,
                generation,
                oid,
                rows,
            } => self.load_search_commit(id, generation, oid, rows),
            Purpose::TakeSide(path, side) => {
                if let Some(conflict) = self
                    .active_tab_mut()
                    .and_then(|tab| tab.conflict.as_mut())
                    .filter(|conflict| conflict.path == path)
                {
                    conflict.saving(true);
                }
                self.op_recorded(
                    format!("resolve {}", path.display()),
                    OpKind::ResolveConflict(path.clone()),
                    Some(Record::new(undo::resolve(path.clone()))),
                    move |r| async move { r.take_side(&path, side).await },
                )
            }
            Purpose::MarkResolved(path) => {
                if let Some(conflict) = self
                    .active_tab_mut()
                    .and_then(|tab| tab.conflict.as_mut())
                    .filter(|conflict| conflict.path == path)
                {
                    conflict.saving(true);
                }
                self.op_recorded(
                    format!("mark {} resolved", path.display()),
                    OpKind::ResolveConflict(path.clone()),
                    Some(Record::new(undo::resolve(path.clone()))),
                    move |r| async move { r.mark_conflict_resolved(&path).await },
                )
            }
            Purpose::Amend { message } => {
                let summary = message
                    .lines()
                    .next()
                    .filter(|l| !l.trim().is_empty())
                    .unwrap_or("message kept")
                    .to_owned();
                self.op_recorded(
                    format!("amend commit: {summary}"),
                    OpKind::Commit,
                    Some(Record::head(gf_git::HeadMode::Soft)),
                    move |r| async move {
                        let opts = gf_git::CommitOptions {
                            amend: true,
                            ..gf_git::CommitOptions::default()
                        };
                        r.commit(&message, &opts).await.map(|_| ())
                    },
                )
            }
        }
    }
}

/// Push options for an explicit destination, or the current branch.
fn push_options(to: Option<&super::PushTo>) -> PushOptions {
    PushOptions {
        remote: to.map(|t| t.remote.clone()),
        branch: to.map(|t| t.refspec.clone()),
        ..PushOptions::default()
    }
}

/// Reset modes offered by the reset dialog, in order (soft first, the
/// default): mode, label, whether it destroys uncommitted work.
pub(super) const RESET_MODES: &[(gf_git::ResetMode, &str, bool)] = &[
    (
        gf_git::ResetMode::Soft,
        "soft — keep the changes, staged",
        false,
    ),
    (
        gf_git::ResetMode::Mixed,
        "mixed — keep the changes, unstaged",
        false,
    ),
    (
        gf_git::ResetMode::Hard,
        "hard — throw away all uncommitted changes",
        true,
    ),
];

/// Input checks that don't need git; branch names are fully validated by
/// git itself when the command runs.
fn validate(purpose: &Purpose, values: &[String]) -> Option<String> {
    let first = values.first().map(String::as_str).unwrap_or_default();
    match purpose {
        Purpose::NewBranch { .. } | Purpose::RenameBranch { .. } | Purpose::NewTag { .. } => {
            if first.is_empty() {
                Some("a name is required".into())
            } else if first.contains(char::is_whitespace) {
                Some("names cannot contain spaces".into())
            } else if first.starts_with('-')
                || first.contains("..")
                || first.ends_with('/')
                || first.ends_with(".lock")
            {
                Some(format!("`{first}` is not a valid name"))
            } else {
                None
            }
        }
        Purpose::LogIn(_) => super::settings::check_login(values),
        Purpose::SetTool(tool) => super::settings::check_tool(*tool, first),
        Purpose::Clone => super::clone::check_clone(values),
        Purpose::Publish { .. } if first.is_empty() => Some("choose a remote".into()),
        Purpose::AddWorktree { .. } if worktree_folder_name(first).is_empty() => {
            Some("a branch is required".into())
        }
        Purpose::AddWorktree { checked_out, .. } if checked_out.iter().any(|b| b == first) => Some(
            format!("{first} is already checked out in another worktree"),
        ),
        Purpose::SetBranchBase { branch, .. } if first == branch => {
            Some(format!("{branch} cannot go into itself"))
        }
        Purpose::SetBranchBase { local, .. }
            if !first.is_empty() && !local.iter().any(|b| b == first) =>
        {
            Some(format!("no local branch {first}"))
        }
        Purpose::TrackLfs if first.is_empty() => Some("enter a pattern such as *.psd".into()),
        Purpose::Squash { .. } if first.is_empty() => {
            Some("the squashed commit needs a summary".into())
        }
        _ => None,
    }
}

/// Branches a new worktree can check out, for the branch field: local ones
/// first, then the remote ones without a local branch (by the name git
/// creates locally). Ones checked out in a tree are listed but cannot be
/// picked: git allows a branch in one tree only.
fn branch_suggestions(
    branches: &[gf_core::model::Branch],
    checked_out: &[String],
) -> Vec<crate::features::dialog::Suggestion> {
    use crate::features::dialog::Suggestion;
    use gf_core::model::RefKind;
    let mut local: Vec<&str> = branches
        .iter()
        .filter(|b| b.name.kind() == RefKind::LocalBranch)
        .map(|b| b.name.short())
        .collect();
    local.sort_unstable();
    let mut remote: Vec<(&str, &str)> = branches
        .iter()
        .filter(|b| b.name.kind() == RefKind::RemoteBranch)
        .filter_map(|b| {
            let full = b.name.short();
            let (_, name) = full.split_once('/')?;
            (name != "HEAD" && !local.contains(&name)).then_some((name, full))
        })
        .collect();
    remote.sort_unstable();
    remote.dedup_by(|a, b| a.0 == b.0);
    local
        .into_iter()
        .map(|name| {
            let busy = checked_out.iter().any(|c| c == name);
            Suggestion::new(name, if busy { "checked out" } else { "" }).disabled(busy)
        })
        .chain(
            remote
                .into_iter()
                .map(|(name, full)| Suggestion::new(name, full)),
        )
        .collect()
}

/// A worktree in op labels: its folder name, not the whole path.
pub(super) fn worktree_label(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Removes trees merged into `base` (and with `delete_branches`, their
/// branches). Each tree is checked again first: still clean and with
/// nothing `base` lacks (see [`gf_git::WorktreeStatus::all_in_base`]) — it
/// may have changed since the dialog opened.
/// Goes on past a failing tree and reports the failures at the end.
pub(super) async fn cleanup_worktrees(
    r: &gf_git::Repository,
    base: &str,
    trees: &[(PathBuf, Option<String>)],
    delete_branches: bool,
) -> gf_core::Result<()> {
    let mut failed = Vec::new();
    for (path, branch) in trees {
        let name = branch.clone().unwrap_or_else(|| worktree_label(path));
        let result = async {
            let s = r.worktree_status(path, Some(base)).await?;
            if !s.is_clean() || !s.all_in_base() {
                return Err(gf_core::GfError::Other(format!(
                    "{name} has new work; left as it is"
                )));
            }
            r.remove_worktree(path, false).await?;
            if delete_branches && let Some(b) = branch {
                // Checked above: all of it is in `base`, if not as the
                // same commits (squashed): forced.
                r.delete_branch(b, true).await?;
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            failed.push(format!("{name}: {e}"));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(gf_core::GfError::Other(failed.join("\n")))
    }
}

/// Folder name for a new worktree of `branch`: `feature/x` → `feature-x`.
pub(super) fn worktree_folder_name(branch: &str) -> String {
    let name: String = branch
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ':' {
                '-'
            } else {
                c
            }
        })
        .collect();
    name.trim_matches(['-', '.']).to_owned()
}

/// Sends the tags a push carries along (none: nothing to do).
async fn push_tags(r: &gf_git::Repository, tags: Option<PendingTags>) -> gf_core::Result<()> {
    match tags {
        Some(t) => r.push_tags(&t.remote, &t.names).await,
        None => Ok(()),
    }
}

fn short_ref(full: &str) -> &str {
    ["refs/heads/", "refs/remotes/", "refs/tags/"]
        .iter()
        .find_map(|p| full.strip_prefix(p))
        .unwrap_or(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_branch_suggestions_list_local_then_remote_only_ones() {
        use gf_core::model::{Branch, RefName};
        let branch = |name: &str| Branch {
            name: RefName::new(name),
            target: "a".repeat(40).parse().expect("oid"),
            upstream: None,
            is_head: false,
        };
        let list = branch_suggestions(
            &[
                branch("refs/remotes/origin/HEAD"),
                branch("refs/remotes/origin/main"),
                branch("refs/remotes/origin/agent/x"),
                branch("refs/heads/main"),
                branch("refs/heads/b"),
            ],
            &["main".to_owned()],
        );
        let shown: Vec<_> = list
            .iter()
            .map(|s| (s.value.as_str(), s.hint.as_str(), s.disabled))
            .collect();
        assert_eq!(
            shown,
            [
                ("b", "", false),
                ("main", "checked out", true),
                ("agent/x", "origin/agent/x", false),
            ]
        );
    }

    #[test]
    fn validates_names() {
        let p = Purpose::NewBranch {
            from: None,
            unborn: false,
        };
        assert!(validate(&p, &["".into()]).is_some());
        assert!(validate(&p, &["has space".into()]).is_some());
        assert!(validate(&p, &["a..b".into()]).is_some());
        assert!(validate(&p, &["feature/ok".into()]).is_none());
        assert!(validate(&Purpose::Stash, &[]).is_none());
    }
}
