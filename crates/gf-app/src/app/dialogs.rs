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

use gf_core::model::{Branch, RefKind};
use gf_git::{PullMode, PushOptions};
use iced::Task;
use iced::widget::operation;

use super::outputs::stash_options;
use super::{App, Message, OpKind};
use crate::features::dialog::{Dialog, FIRST_FIELD_ID, Outcome, Severity};
use crate::features::diff_view::Action;

/// What a dialog is for; carried through the dialog and acted on at submit.
#[derive(Debug, Clone)]
pub enum Purpose {
    Discard {
        tracked: Vec<PathBuf>,
        untracked: Vec<PathBuf>,
    },
    ApplyPatches {
        patches: Vec<String>,
        action: Action,
        description: String,
    },
    NewBranch {
        from: Option<String>,
    },
    RenameBranch {
        old: String,
    },
    DeleteBranch {
        name: String,
    },
    DeleteRemoteBranch {
        remote: String,
        branch: String,
    },
    NewTag {
        at: Option<String>,
    },
    DeleteTag(String),
    Stash,
    StashDrop(usize),
    Publish {
        branch: String,
    },
    ForcePush {
        branch: Option<String>,
        to: Option<super::PushTo>,
    },
    PullStrategy,
    CheckoutDetached(String),
    Checkout {
        name: String,
        remote: bool,
    },
    StashApply(usize),
    StashPop(usize),
    Pull,
    Push {
        branch: String,
        /// Set for a branch that is not checked out.
        to: Option<super::PushTo>,
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
        base: gf_core::model::Oid,
        items: Vec<gf_git::TodoItem>,
    },
    BisectStart {
        good: gf_core::model::Oid,
    },
    BisectReset,
    SkipOperation,
    AbortOperation,
    Undo(crate::repo_tab::UndoEntry),
    ResolveBlocks {
        path: PathBuf,
        segments: Vec<gf_git::conflict::Segment>,
        choices: Vec<Option<gf_git::conflict::Choice>>,
    },
    TakeSide(PathBuf, gf_git::Side),
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
    checks: Vec<(String, bool)>,
    then: Purpose,
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
            checks: Vec::new(),
            then,
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
            checks: Vec::new(),
            then,
        }
    }

    /// A change that must always be asked (no "don't ask again").
    pub fn always(mut self) -> Self {
        self.key = None;
        self
    }

    pub fn check(mut self, label: impl Into<String>, value: bool) -> Self {
        self.checks.push((label.into(), value));
        self
    }
}

impl App {
    /// Asks before running `c.then` — the one way to confirm a delete or
    /// change. Skipped (runs right away) when the user ticked "don't ask
    /// again" for this change before.
    pub(super) fn confirm(&mut self, c: Confirm) -> Task<Message> {
        if let Some(key) = c.key {
            if c.severity == Severity::Change && self.settings.confirm.is_skipped(key) {
                let checks = c.checks.iter().map(|(_, v)| *v).collect();
                return self.run_purpose(c.then, Vec::new(), checks);
            }
        }
        let mut dialog = Dialog::confirm(c.title, c.body, c.label, c.then).severity(c.severity);
        for (label, value) in c.checks {
            dialog = dialog.check(label, value);
        }
        if let Some(key) = c.key {
            dialog = dialog.remember_as(key);
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

    pub(super) fn new_branch_dialog(&mut self, from: Option<String>) -> Task<Message> {
        let start = from.as_deref().map_or_else(
            || "the current commit (HEAD)".to_owned(),
            |f| format!("`{}`", short_ref(f)),
        );
        let suggested = from
            .as_deref()
            .and_then(|f| f.strip_prefix("refs/remotes/"))
            .and_then(|f| f.split_once('/'))
            .map(|(_, b)| b.to_owned())
            .unwrap_or_default();
        self.open_dialog(
            Dialog::confirm(
                "New branch",
                vec![format!("Start at {start}.")],
                "Create",
                Purpose::NewBranch { from },
            )
            .field("name", suggested, "feature/my-change")
            .check("switch to the new branch", true),
        )
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

    pub(super) fn delete_branch_dialog(&mut self, b: &Branch) -> Task<Message> {
        let short = b.name.short().to_owned();
        if b.name.kind() == RefKind::RemoteBranch {
            let Some((remote, branch)) = short.split_once('/') else {
                return Task::none();
            };
            return self.confirm(Confirm::delete(
                "Delete remote branch",
                vec![
                    format!("Delete `{branch}` on remote `{remote}`?"),
                    "This affects everyone using that remote.".into(),
                ],
                "Delete on remote",
                Purpose::DeleteRemoteBranch {
                    remote: remote.to_owned(),
                    branch: branch.to_owned(),
                },
            ));
        }
        self.confirm(
            Confirm::delete(
                "Delete branch",
                vec![
                    format!("Delete local branch `{short}`?"),
                    "git refuses if it has commits not merged anywhere —".into(),
                    "tick force to delete anyway.".into(),
                ],
                "Delete",
                Purpose::DeleteBranch { name: short },
            )
            .check("force (also delete unmerged commits)", false),
        )
    }

    pub(super) fn on_dialog_outcome(&mut self, outcome: Outcome<Purpose>) -> Task<Message> {
        let (purpose, values, checks, dont_ask_again) = match outcome {
            Outcome::Submitted {
                purpose,
                values,
                checks,
                dont_ask_again,
            } => (purpose, values, checks, dont_ask_again),
            Outcome::Cancelled { purpose } => {
                self.dialog = None;
                // The commit box waits for the amend to run; release it.
                if matches!(purpose, Purpose::Amend { .. }) {
                    if let Some(tab) = self.active_tab_mut() {
                        tab.commit.commit_failed();
                    }
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
                crate::features::command_log::NoteKind::Info,
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
            Purpose::Discard { tracked, untracked } => {
                self.op("discard changes", OpKind::Generic, move |r| async move {
                    r.discard(&tracked, &untracked).await
                })
            }
            Purpose::ApplyPatches {
                patches,
                action,
                description,
            } => self.apply_patches(patches, action, description),
            Purpose::NewBranch { from } => {
                let name = value(0);
                let checkout = check(0);
                self.op(
                    format!("create branch {name}"),
                    OpKind::Generic,
                    move |r| async move { r.create_branch(&name, from.as_deref(), checkout).await },
                )
            }
            Purpose::RenameBranch { old } => {
                let new = value(0);
                self.op(
                    format!("rename {old} → {new}"),
                    OpKind::Generic,
                    move |r| async move { r.rename_branch(&old, &new).await },
                )
            }
            Purpose::DeleteBranch { name } => {
                let force = check(0);
                self.op(
                    format!("delete branch {name}"),
                    OpKind::Generic,
                    move |r| async move { r.delete_branch(&name, force).await },
                )
            }
            Purpose::DeleteRemoteBranch { remote, branch } => self.op(
                format!("delete {remote}/{branch}"),
                OpKind::Generic,
                move |r| async move { r.delete_remote_branch(&remote, &branch).await },
            ),
            Purpose::NewTag { at } => {
                let name = value(0);
                let message = value(1);
                self.op(
                    format!("create tag {name}"),
                    OpKind::Generic,
                    move |r| async move {
                        let msg = (!message.is_empty()).then_some(message.as_str());
                        r.create_tag(&name, at.as_deref(), msg).await
                    },
                )
            }
            Purpose::DeleteTag(name) => self.op(
                format!("delete tag {name}"),
                OpKind::Generic,
                move |r| async move { r.delete_tag(&name).await },
            ),
            Purpose::Stash => {
                let opts = stash_options(value(0), check(0), check(1));
                self.op("stash changes", OpKind::Generic, move |r| async move {
                    r.stash_push(&opts).await
                })
            }
            Purpose::StashDrop(i) => self.op(
                format!("drop stash@{{{i}}}"),
                OpKind::Generic,
                move |r| async move { r.stash_drop(i).await },
            ),
            Purpose::Publish { branch } => {
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
                    move |r, run| async move { r.push(&opts, run).await },
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
            Purpose::CheckoutDetached(rev) => self.op(
                // By characters: a revision is not always an ASCII id.
                format!("check out {}", rev.chars().take(7).collect::<String>()),
                OpKind::Generic,
                move |r| async move { r.checkout_detached(&rev).await },
            ),
            Purpose::Checkout { name, remote } => {
                let label = if remote {
                    format!("check out {name} as a tracking branch")
                } else {
                    format!("switch to {name}")
                };
                self.op(label, OpKind::Generic, move |r| async move {
                    r.switch_branch(&name, remote).await
                })
            }
            Purpose::StashApply(i) => self.op(
                format!("apply stash@{{{i}}}"),
                OpKind::Generic,
                move |r| async move { r.stash_apply(i).await },
            ),
            Purpose::StashPop(i) => self.op(
                format!("pop stash@{{{i}}}"),
                OpKind::Generic,
                move |r| async move { r.stash_pop(i).await },
            ),
            Purpose::Pull => self.pull(PullMode::Config),
            Purpose::Push { branch, to } => {
                let opts = push_options(to.as_ref());
                self.sync_op(
                    format!("push {branch}"),
                    OpKind::Push {
                        branch: Some(branch),
                        to,
                    },
                    move |r, run| async move { r.push(&opts, run).await },
                )
            }
            Purpose::PushTag { remote, name } => self.op(
                format!("push tag {name}"),
                OpKind::Generic,
                move |r| async move { r.push_tag(&remote, &name).await },
            ),
            Purpose::Merge { branch } => self.run_merge(branch, check(0)),
            Purpose::RebaseOnto { onto } => self.run_rebase_onto(onto),
            Purpose::CherryPick(oids) => self.run_cherry_pick(oids),
            Purpose::Revert(oids) => self.run_revert(oids),
            Purpose::RebaseInteractive { base, items } => self.run_rebase_interactive(base, items),
            Purpose::BisectStart { good } => self
                .bisect_step(move |r| async move { r.bisect_start("HEAD", good.as_str()).await }),
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
            Purpose::AbortOperation => self.op("abort", OpKind::Generic, |r| async move {
                r.abort_operation().await
            }),
            Purpose::Undo(entry) => self.run_undo(entry),
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
                    OpKind::Generic,
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
            Purpose::TakeSide(path, side) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.conflict = None;
                }
                self.op(
                    format!("resolve {}", path.display()),
                    OpKind::Generic,
                    move |r| async move { r.take_side(&path, side).await },
                )
            }
            Purpose::MarkResolved(path) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.conflict = None;
                }
                self.op(
                    format!("mark {} resolved", path.display()),
                    OpKind::Generic,
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
                let undo = self.undo_point(&format!("amend: {summary}"), true);
                self.op_with_undo(
                    format!("amend commit: {summary}"),
                    OpKind::Commit,
                    undo,
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
        _ => None,
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
        let p = Purpose::NewBranch { from: None };
        assert!(validate(&p, &["".into()]).is_some());
        assert!(validate(&p, &["has space".into()]).is_some());
        assert!(validate(&p, &["a..b".into()]).is_some());
        assert!(validate(&p, &["feature/ok".into()]).is_none());
        assert!(validate(&Purpose::Stash, &[]).is_none());
    }
}
