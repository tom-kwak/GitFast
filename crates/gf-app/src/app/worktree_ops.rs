//! What the worktrees view asks for (`worktrees::Output`): showing a
//! tree's work, opening it elsewhere, and the confirmed changes (remove,
//! lock, prune, clean up merged trees, merge into the base, set the base).

use std::path::{Path, PathBuf};

use iced::Task;

use super::dialogs::{Confirm, Purpose, worktree_label};
use super::{App, Message, Shortcut};
use crate::features::command_log::NoteKind;
use crate::features::dialog::{Dialog, Suggestion};
use crate::features::diff_view::DiffSource;
use crate::features::{sidebar, worktrees};
use crate::widgets::format::tilde;

impl App {
    pub(super) fn on_worktrees(&mut self, out: worktrees::Output) -> Task<Message> {
        use worktrees::Output;
        match out {
            Output::ShowRevision(rev) => self.on_sidebar(sidebar::Output::ShowRevision(Some(rev))),
            Output::OpenRepo(path) => self.update(Message::Open(path)),
            Output::Compare { from, to, count } => self.show_diff(DiffSource::Range {
                from: from.to_string(),
                to,
                count,
                contiguous: true,
            }),
            Output::Editor(path) => {
                // The editor opening is the feedback; only failures speak.
                let editor = self.settings.worktrees.editor.clone();
                if let Err(e) = crate::platform::open_in_editor(&editor, &path) {
                    self.flash(
                        NoteKind::Error,
                        format!("could not run `{editor}`: {e} — :editor <cmd> picks another"),
                    );
                }
                Task::none()
            }
            Output::Terminal(path) => {
                let app = self.settings.worktrees.terminal.clone();
                if let Err(e) = crate::platform::open_terminal(app.as_deref(), &path) {
                    self.flash(
                        NoteKind::Error,
                        format!("could not open a terminal: {e} — :terminal <app> picks another"),
                    );
                }
                Task::none()
            }
            Output::Reveal(path) => Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || crate::platform::reveal(&path))
                        .await
                        .map_err(|e| e.to_string())?
                        .map_err(|e| e.to_string())
                },
                Message::PathRevealed,
            ),
            Output::Copy(path) => {
                self.flash(NoteKind::Info, format!("copied {}", tilde(&path)));
                iced::clipboard::write(path.display().to_string())
            }
            Output::Lock(path, locked) => {
                let (title, verb, line) = if locked {
                    (
                        "Lock worktree",
                        "Lock",
                        "git then refuses to remove or prune it.",
                    )
                } else {
                    (
                        "Unlock worktree",
                        "Unlock",
                        "It can be removed or pruned again.",
                    )
                };
                self.confirm(Confirm::change(
                    "worktree-lock",
                    title,
                    vec![format!("{verb} {}?", worktree_label(&path)), line.into()],
                    verb,
                    Purpose::LockWorktree(path, locked),
                ))
            }
            Output::Remove(removal) => self.remove_worktree_dialog(removal),
            Output::Prune => self.confirm(Confirm::change(
                "worktree-prune",
                "Prune worktrees",
                vec!["Forget the trees whose folder is gone? Their branches stay.".into()],
                "Prune",
                Purpose::PruneWorktrees,
            )),
            Output::Cleanup { base, trees } => {
                let review = trees
                    .iter()
                    .map(|(p, b)| b.clone().unwrap_or_else(|| worktree_label(p)))
                    .collect();
                self.confirm(
                    Confirm::delete(
                        "Remove merged worktrees",
                        vec![format!(
                            "Remove {} clean tree(s) already merged into {base}?",
                            trees.len()
                        )],
                        "Remove",
                        Purpose::CleanupWorktrees { base, trees },
                    )
                    .review(review)
                    .check("also delete their branches", true),
                )
            }
            Output::Merge {
                branch,
                base,
                into,
                real,
                count,
            } => {
                let here = self
                    .active_tab()
                    .is_some_and(|t| same_folder(t.repo.cwd(), &into, real.as_deref()));
                let place = if here {
                    "It runs here, in this tab's tree.".to_owned()
                } else {
                    format!("It runs in the tree of {base}, which opens in its tab.")
                };
                let commits = if count == 1 {
                    "1 commit".to_owned()
                } else {
                    format!("{count} commits")
                };
                self.confirm(
                    Confirm::change(
                        "merge",
                        "Merge",
                        vec![
                            format!("Merge {branch} into {base}? ({commits})"),
                            place,
                            "Fast-forwards when possible; conflicts stop there for you to resolve."
                                .into(),
                        ],
                        "Merge",
                        Purpose::MergeIntoBase { branch, into, real },
                    )
                    .check("always create a merge commit (--no-ff)", false),
                )
            }
            Output::SetBase {
                branch,
                base,
                guessed,
            } => self.base_branch_dialog(branch, base, guessed),
            Output::New => self.new_worktree_dialog(),
            Output::Collapse => self.on_shortcut(Shortcut::ToggleRefs),
            Output::Menu { show, menu } => {
                let task = show.map_or_else(Task::none, |rev| {
                    self.on_sidebar(sidebar::Output::ShowRevision(Some(rev)))
                });
                Task::batch([task, self.open_menu(menu.map(Message::Worktrees))])
            }
        }
    }

    /// Merges `branch` in the tab of the tree at `into` (opening it first
    /// when no tab shows it): conflicts then stop where they are resolved.
    pub(super) fn merge_in_tree(
        &mut self,
        branch: String,
        into: PathBuf,
        real: Option<&Path>,
        no_ff: bool,
    ) -> Task<Message> {
        let tab = self
            .tabs
            .iter()
            .position(|t| same_folder(t.repo.cwd(), &into, real));
        match tab {
            Some(i) => {
                let select = self.select_tab(i);
                Task::batch([select, self.run_merge(branch, no_ff)])
            }
            None => {
                self.merge_after_open = Some(MergeAfterOpen {
                    path: into.clone(),
                    branch,
                    no_ff,
                });
                self.update(Message::Open(into))
            }
        }
    }

    /// Asks which local branch `branch` goes back into, with the local
    /// branches to pick from.
    fn base_branch_dialog(
        &mut self,
        branch: String,
        base: Option<String>,
        guessed: bool,
    ) -> Task<Message> {
        let mut local: Vec<String> = self
            .active_tab()
            .and_then(|t| t.snapshot.as_ref())
            .map(|snap| {
                snap.branches
                    .iter()
                    .filter(|b| b.name.kind() == gf_core::model::RefKind::LocalBranch)
                    .map(|b| b.name.short().to_owned())
                    .filter(|b| *b != branch)
                    .collect()
            })
            .unwrap_or_default();
        local.sort_unstable();
        let suggestions = local
            .iter()
            .map(|b| {
                Suggestion::new(
                    b.as_str(),
                    if Some(b) == base.as_ref() { "now" } else { "" },
                )
            })
            .collect();
        let mut lines = vec![format!(
            "The branch {branch} came from: merges go back into it."
        )];
        if guessed && let Some(base) = &base {
            lines.push(format!(
                "{base} is a guess (the nearest of the usual branches): nothing recorded where it came from."
            ));
        }
        lines.push("Empty: guess it again.".into());
        let dialog = Dialog::confirm(
            "Base branch",
            lines,
            "Set",
            Purpose::SetBranchBase { branch, local },
        )
        .field("base", base.unwrap_or_default(), "a local branch")
        .suggest(suggestions, "no such local branch");
        self.open_dialog(dialog)
    }

    /// Asks before removing a working tree, saying only what applies to it:
    /// uncommitted changes (and the force needed to drop them), commits on
    /// no branch.
    fn remove_worktree_dialog(&mut self, removal: worktrees::Removal) -> Task<Message> {
        let worktrees::Removal {
            path,
            name,
            detached,
            changed,
        } = removal;
        let mut lines = vec![format!("Remove the worktree of {name}?")];
        lines.push(if detached {
            "Its folder is deleted. It is on no branch: commits only it has are lost.".into()
        } else {
            format!("Its folder is deleted; the branch {name} stays.")
        });
        // Unknown: git's own check (it refuses with changes) stays.
        let dirty = changed.is_some_and(|n| n > 0);
        if let Some(n) = changed.filter(|n| *n > 0) {
            lines.push(format!("{n} uncommitted change(s) there are lost too."));
        }
        let confirm = Confirm::delete(
            "Remove worktree",
            lines,
            "Remove",
            Purpose::RemoveWorktree { path, force: dirty },
        );
        self.confirm(confirm)
    }
}

/// A merge waiting for its tree's tab ([`App::merge_in_tree`]).
#[derive(Debug, Clone)]
pub(super) struct MergeAfterOpen {
    pub path: PathBuf,
    pub branch: String,
    pub no_ff: bool,
}

/// `cwd` (a tab's folder, resolved) is the tree at `path` (`real`: resolved).
fn same_folder(cwd: &Path, path: &Path, real: Option<&Path>) -> bool {
    cwd == path || real == Some(cwd)
}
