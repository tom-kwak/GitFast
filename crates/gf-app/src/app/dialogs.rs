//! Dialog purposes and what happens when a dialog is submitted.

use std::path::PathBuf;

use gf_core::model::{Branch, RefKind};
use gf_git::{PullMode, PushOptions};
use iced::Task;
use iced::widget::operation;

use super::outputs::stash_options;
use super::{App, Message, OpKind};
use crate::features::dialog::{Dialog, FIRST_FIELD_ID, Outcome};
use crate::features::diff_view::Action;
use crate::features::prompt::PROMPT_ID;

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
    },
    PullStrategy,
    CheckoutDetached(String),
}

impl App {
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
            return self.open_dialog(
                Dialog::confirm(
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
                )
                .danger(),
            );
        }
        self.open_dialog(
            Dialog::confirm(
                "Delete branch",
                vec![
                    format!("Delete local branch `{short}`?"),
                    "git refuses if it has commits not merged anywhere —".into(),
                    "tick force to delete anyway.".into(),
                ],
                "Delete",
                Purpose::DeleteBranch { name: short },
            )
            .check("force (also delete unmerged commits)", false)
            .danger(),
        )
    }

    pub(super) fn on_dialog_outcome(&mut self, outcome: Outcome<Purpose>) -> Task<Message> {
        let Outcome::Submitted {
            purpose,
            values,
            checks,
        } = outcome
        else {
            self.dialog = None;
            return operation::focus(PROMPT_ID);
        };
        let value = |i: usize| values.get(i).cloned().unwrap_or_default();
        let check = |i: usize| checks.get(i).copied().unwrap_or(false);

        // Validate before closing so the user can fix the input in place.
        if let Some(error) = validate(&purpose, &values) {
            if let Some(d) = self.dialog.as_mut() {
                d.error = Some(error);
            }
            return Task::none();
        }
        self.dialog = None;

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
                self.sync_op(
                    label,
                    OpKind::Push {
                        branch: Some(branch),
                    },
                    move |r, run| async move { r.push(&opts, run).await },
                )
            }
            Purpose::ForcePush { branch } => {
                let opts = PushOptions {
                    force_with_lease: true,
                    ..PushOptions::default()
                };
                self.sync_op(
                    "force push (with lease)",
                    OpKind::Push { branch },
                    move |r, run| async move { r.push(&opts, run).await },
                )
            }
            Purpose::PullStrategy => self.pull(if check(0) {
                PullMode::Rebase
            } else {
                PullMode::Merge
            }),
            Purpose::CheckoutDetached(rev) => self.op(
                format!("check out {}", &rev[..rev.len().min(7)]),
                OpKind::Generic,
                move |r| async move { r.checkout_detached(&rev).await },
            ),
        }
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
