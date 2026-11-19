//! Dialogs: input dialogs (new branch, tag, stash…) and the shared
//! confirmation used before every delete or change.
//!
//! # Confirming an action (reuse this for every new feature)
//!
//! ```ignore
//! self.confirm(
//!     Confirm::change("pull", "Pull", vec!["…what will happen…".into()], "Pull", Purpose::Pull)
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
    PullStrategy,
    CheckoutDetached(String),
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
    Pull,
    Push {
        branch: String,
        /// Set for a branch that is not checked out.
        to: Option<super::PushTo>,
        /// Tags the remote does not have, sent along (checked-out branch).
        tags: Option<PendingTags>,
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
    /// Remove a repository from the recent list (the folder is untouched).
    ForgetRecent(PathBuf),
    SkipOperation,
    AbortOperation,
    /// Undo (or redo) the next step; the check box stashes first.
    Undo {
        redo: bool,
        /// "Undo anyway": overwrite what changed since the step.
        force: bool,
    },
    ResolveBlocks {
        path: PathBuf,
        segments: Vec<gf_git::conflict::Segment>,
        choices: Vec<Option<gf_git::conflict::Choice>>,
    },
    TakeSide(PathBuf, gf_git::Side),
    /// Load the history down to a far commit, then show it.
    RevealCommit {
        oid: gf_core::model::Oid,
        at: super::reveal::Located,
    },
    MarkResolved(PathBuf),
    AddWorktree,
    RemoveWorktree(PathBuf),
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

    pub(super) fn new_worktree_dialog(&mut self) -> Task<Message> {
        let suggested = self
            .active_tab()
            .and_then(|t| {
                t.repo
                    .cwd()
                    .parent()
                    .map(|p| p.join(format!("{}-wt", t.name())))
            })
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.open_dialog(
            Dialog::confirm(
                "Add worktree",
                vec!["Check a branch out in another folder, side by side with this one.".into()],
                "Add",
                Purpose::AddWorktree,
            )
            .field("folder", suggested, "/path/to/new/folder")
            .field("branch", "", "feature/x")
            .check("create the branch (from HEAD)", true),
        )
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
            Purpose::ForgetRecent(path) => self.forget_recent(&path),
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
                    OpKind::Push { branch, to },
                    move |r, run| async move { r.push(&opts, run).await },
                )
            }
            Purpose::PullStrategy => self.pull(if check(0) {
                PullMode::Rebase
            } else {
                PullMode::Merge
            }),
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
            Purpose::Pull => self.pull(PullMode::Config),
            Purpose::Push { branch, to, tags } => {
                let opts = push_options(to.as_ref());
                self.sync_op(
                    format!("push {branch}"),
                    OpKind::Push {
                        branch: Some(branch),
                        to,
                    },
                    move |r, run| async move {
                        r.push(&opts, run).await?;
                        push_tags(&r, tags).await
                    },
                )
            }
            Purpose::PushTag { remote, name } => self.op(
                format!("push tag {name}"),
                OpKind::Generic,
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
                    tab.pending = None;
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
            Purpose::AddWorktree => {
                let folder = crate::widgets::format::expand_tilde(&value(0));
                let branch = value(1);
                let new_branch = check(0);
                self.op(
                    format!("add worktree {branch}"),
                    OpKind::Generic,
                    move |r| async move { r.add_worktree(&folder, &branch, new_branch).await },
                )
            }
            Purpose::RemoveWorktree(path) => {
                let force = check(0);
                self.op(
                    format!("remove worktree {}", path.display()),
                    OpKind::RemoveWorktree(path.clone()),
                    move |r| async move { r.remove_worktree(&path, force).await },
                )
            }
            Purpose::UpdateSubmodules => {
                self.op("update submodules", OpKind::Generic, |r| async move {
                    r.update_submodules().await
                })
            }
            Purpose::TrackLfs => {
                let pattern = value(0);
                self.op(
                    format!("lfs track {pattern}"),
                    OpKind::Generic,
                    move |r| async move { r.lfs_track(&pattern).await },
                )
            }
            Purpose::UntrackLfs(pattern) => self.op(
                format!("lfs untrack {pattern}"),
                OpKind::Generic,
                move |r| async move { r.lfs_untrack(&pattern).await },
            ),
            Purpose::LfsPull => self.op("lfs pull", OpKind::Generic, |r| async move {
                r.lfs_pull().await
            }),
            Purpose::ResolveBlocks {
                path,
                segments,
                choices,
            } => self.run_resolve(path, segments, choices),
            Purpose::RevealCommit { oid, at } => self.show_located(oid, at),
            Purpose::TakeSide(path, side) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.conflict = None;
                }
                self.op_recorded(
                    format!("resolve {}", path.display()),
                    OpKind::Generic,
                    Some(Record::new(undo::resolve(path.clone()))),
                    move |r| async move { r.take_side(&path, side).await },
                )
            }
            Purpose::MarkResolved(path) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.conflict = None;
                }
                self.op_recorded(
                    format!("mark {} resolved", path.display()),
                    OpKind::Generic,
                    Some(Record::new(undo::resolve(path.clone()))),
                    move |r| async move { r.stage(&[path]).await },
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
        Purpose::Publish { .. } if first.is_empty() => Some("choose a remote".into()),
        Purpose::AddWorktree if first.is_empty() => Some("choose a folder".into()),
        Purpose::AddWorktree if values.get(1).is_none_or(|b| b.is_empty()) => {
            Some("a branch is required".into())
        }
        Purpose::TrackLfs if first.is_empty() => Some("enter a pattern such as *.psd".into()),
        Purpose::Squash { .. } if first.is_empty() => {
            Some("the squashed commit needs a summary".into())
        }
        _ => None,
    }
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
