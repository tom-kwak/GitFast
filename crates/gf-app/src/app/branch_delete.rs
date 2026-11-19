//! Confirms a set of selected refs and deletes branches in batches.

use std::collections::{BTreeMap, HashSet};

use gf_core::GfError;
use gf_core::model::{Branch, RefKind};
use iced::Task;

use super::dialogs::{Confirm, Purpose};
use super::ops::Record;
use super::{App, Message, OpKind};
use crate::features::dialog::Dialog;
use crate::git_actions::error_excerpt;
use crate::undo;

#[derive(Debug, Clone, Default)]
pub struct BranchDeletion {
    local: Vec<String>,
    remote: BTreeMap<String, Vec<String>>,
}

impl BranchDeletion {
    fn len(&self) -> usize {
        self.local.len() + self.remote.values().map(Vec::len).sum::<usize>()
    }
}

impl App {
    pub(super) fn delete_branches_dialog(&mut self, branches: &[Branch]) -> Task<Message> {
        let Some(snap) = self.active_tab().and_then(|tab| tab.snapshot.as_deref()) else {
            return Task::none();
        };
        let mut deletion = BranchDeletion::default();
        let mut review = Vec::new();
        let mut skipped = 0;
        let checked_out: HashSet<_> = snap
            .worktrees
            .iter()
            .filter_map(|worktree| worktree.branch.as_deref())
            .collect();
        for b in branches {
            let name = b.name.short();
            match b.name.kind() {
                RefKind::LocalBranch if b.is_head || checked_out.contains(name) => {
                    review.push(format!("  Keep {name} (checked out)"));
                    skipped += 1;
                }
                RefKind::LocalBranch => {
                    review.push(format!("  Local: {name}"));
                    deletion.local.push(name.to_owned());
                }
                RefKind::RemoteBranch => {
                    let remote = gf_git::remote_branch(&snap.remotes, b.name.as_str());
                    if let Some((remote, branch)) = remote {
                        review.push(format!("  On remote {remote}: {branch}"));
                        deletion
                            .remote
                            .entry(remote.to_owned())
                            .or_default()
                            .push(branch);
                    } else {
                        review.push(format!("  Keep {name} (no unique remote branch mapping)"));
                        skipped += 1;
                    }
                }
                _ => {}
            }
        }

        let n = deletion.len();
        if n == 0 {
            return self.open_dialog(
                Dialog::notice(
                    "No branches to delete",
                    vec!["Check out another branch first, or select other branches.".into()],
                    Purpose::Failure,
                )
                .review(review),
            );
        }
        let mut body = vec![format!("Branches to delete: {n}")];
        if skipped > 0 {
            body.push(format!("{skipped} selected branch(es) will be kept."));
        }
        let has_local = !deletion.local.is_empty();
        let has_remote = !deletion.remote.is_empty();
        if has_remote {
            body.push("Remote deletions affect everyone using those remotes.".into());
            if has_local {
                body.push("⌘Z can restore deleted local branches.".into());
            }
        }
        let mut confirmation = Confirm::delete(
            "Delete selected branches",
            body,
            format!("Delete {n}"),
            Purpose::DeleteBranches(deletion),
        )
        .review(review);
        if has_local {
            confirmation =
                confirmation.check("force local deletion (include unmerged commits)", false);
            if !has_remote {
                confirmation = confirmation.undoable();
            }
        }
        self.confirm(confirmation)
    }

    pub(super) fn run_delete_branches(
        &mut self,
        deletion: BranchDeletion,
        force: bool,
    ) -> Task<Message> {
        let Some(git_dir) = self.active_tab().map(|t| t.repo.handle().git_dir.clone()) else {
            return Task::none();
        };
        if !self.deleting_branches.insert(git_dir.clone()) {
            self.flash(
                crate::features::command_log::NoteKind::Info,
                "Branch deletion already in progress",
            );
            return Task::none();
        }
        let label = format!("delete {} branches", deletion.len());
        let record = (!deletion.local.is_empty()).then(|| {
            let names: Vec<_> = deletion.local.iter().map(String::as_str).collect();
            Record::new(undo::branches(&names, false)).required()
        });
        self.op_recorded(label, OpKind::DeleteBranches(git_dir), record, move |repo| async move {
            let mut failures = Vec::new();
            if let Err(e) = repo.delete_branches(&deletion.local, force).await {
                failures.push(format!("Local branches:\n{}", error_excerpt(&e.to_string(), 4).join("\n")));
            }
            for (remote, branches) in deletion.remote {
                if let Err(e) = repo.delete_remote_branches(&remote, &branches).await {
                    failures.push(format!("Remote {remote}:\n{}", error_excerpt(&e.to_string(), 4).join("\n")));
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(GfError::Other(format!(
                    "Some selected branches could not be deleted. Check Refs for the remaining branches.\n{}",
                    failures.join("\n")
                )))
            }
        })
    }
}
