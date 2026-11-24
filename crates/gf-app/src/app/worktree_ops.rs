//! What the worktrees view asks for (`worktrees::Output`): showing a
//! tree's work, opening it elsewhere, and the confirmed changes (remove,
//! lock, prune, clean up merged trees, merge into the base, set the base).

use std::path::{Path, PathBuf};

use iced::Task;

use super::dialogs::{Confirm, Purpose, worktree_label};
use super::{App, Message, OpKind, RepoId, Shortcut};
use crate::features::command_log::NoteKind;
use crate::features::dialog::{Dialog, Suggestion};
use crate::features::diff_view::DiffSource;
use crate::features::{sidebar, worktrees};
use crate::widgets::format::{self, tilde};

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
                        "It can't be removed or pruned until you unlock it.",
                    )
                } else {
                    (
                        "Unlock worktree",
                        "Unlock",
                        "It can be removed and pruned again.",
                    )
                };
                self.confirm(Confirm::change(
                    "worktree-lock",
                    title,
                    vec![format!("{verb} {}?", tilde(&path)), line.into()],
                    verb,
                    Purpose::LockWorktree(path, locked),
                ))
            }
            Output::Remove(removal) => self.remove_worktree_dialog(removal),
            Output::Prune => self.confirm(Confirm::change(
                "worktree-prune",
                "Prune worktrees",
                vec![
                    "Clean up worktrees whose folder was deleted?".into(),
                    "Their branches stay.".into(),
                ],
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
                        vec![
                            format!(
                                "Remove {} merged into {base}?",
                                format::count(trees.len() as u64, "worktree")
                            ),
                            "Only ones with no uncommitted changes.".into(),
                        ],
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
                from,
            } => {
                let place = self.runs_where(&into, real.as_deref());
                let commits = format::count(count, "commit");
                let clean = from.map(|tree| CleanAfterMerge {
                    tree,
                    branch: branch.clone(),
                    base: base.clone(),
                });
                let offer_clean = clean.is_some();
                let mut confirm = Confirm::change(
                    "merge",
                    "Merge",
                    vec![
                        format!("Merge {branch} into {base} ({commits})?"),
                        place,
                        "Fast-forwards when it can; conflicts stop for you to resolve.".into(),
                    ],
                    "Merge",
                    Purpose::MergeIntoBase {
                        branch: branch.clone(),
                        into,
                        real,
                        clean,
                    },
                )
                .check("always create a merge commit (--no-ff)", false);
                if offer_clean {
                    // Off by default: removing is not undone with ⌘Z.
                    confirm = confirm.check(
                        format!("then remove the worktree and branch {branch}"),
                        false,
                    );
                }
                self.confirm(confirm)
            }
            Output::SetBase {
                branch,
                base,
                guessed,
            } => self.base_branch_dialog(branch, base, guessed),
            Output::Update {
                branch,
                base,
                tree,
                real,
                behind,
            } => {
                let Some(tab) = self.active_tab() else {
                    return Task::none();
                };
                let repo = tab.repo.clone();
                let update = UpdateFromBase {
                    branch,
                    base,
                    tree,
                    real,
                    behind,
                };
                // Read where it runs: that tree's config decides.
                Task::perform(
                    async move {
                        let rebase = repo
                            .pull_rebases(&update.tree, &update.branch)
                            .await
                            .unwrap_or(false);
                        (update, rebase)
                    },
                    |(update, rebase)| Message::UpdateModeRead(update, rebase),
                )
            }
            Output::New => self.new_worktree_dialog(None),
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
    ///
    /// With `clean`, a merge that ends without conflicts goes on to remove
    /// the merged tree and its branch (see [`CleanAfterMerge`]).
    pub(super) fn merge_in_tree(
        &mut self,
        branch: String,
        into: PathBuf,
        real: Option<&Path>,
        no_ff: bool,
        clean: Option<CleanAfterMerge>,
    ) -> Task<Message> {
        let what = Integrate::Merge {
            branch,
            no_ff,
            clean,
        };
        self.integrate_in_tree(into, real, what)
    }

    /// Runs `what` in the tab of the tree at `path` (opening it first when
    /// no tab shows it), so conflicts stop where they are resolved.
    pub(super) fn integrate_in_tree(
        &mut self,
        path: PathBuf,
        real: Option<&Path>,
        what: Integrate,
    ) -> Task<Message> {
        let tab = self
            .tabs
            .iter()
            .position(|t| same_folder(t.repo.cwd(), &path, real));
        match tab {
            Some(i) => {
                let select = self.select_tab(i);
                Task::batch([select, self.integrate_here(what)])
            }
            None => {
                self.integrate_after_open = Some(IntegrateAfterOpen {
                    path: path.clone(),
                    what,
                });
                self.update(Message::Open(path))
            }
        }
    }

    /// Says where [`Self::integrate_in_tree`] runs for the tree at `path`.
    fn runs_where(&self, path: &Path, real: Option<&Path>) -> String {
        let shows = |t: &crate::repo_tab::RepoTab| same_folder(t.repo.cwd(), path, real);
        if self.active_tab().is_some_and(shows) {
            "Runs here, in this tab.".to_owned()
        } else if self.tabs.iter().any(shows) {
            format!("Runs in {}, in its tab.", tilde(path))
        } else {
            format!("Runs in {}, which opens in a tab.", tilde(path))
        }
    }

    /// Runs `what` in the active tab.
    pub(super) fn integrate_here(&mut self, what: Integrate) -> Task<Message> {
        match what {
            Integrate::Merge {
                branch,
                no_ff,
                clean,
            } => self.merge_then_clean(branch, no_ff, clean),
            Integrate::Rebase { onto } => self.run_rebase_onto(onto),
        }
    }

    /// Asks before bringing `base`'s new commits into the tree of
    /// `branch`, merging or rebasing as `git pull` there would.
    pub(super) fn on_update_mode(&mut self, update: UpdateFromBase, rebase: bool) -> Task<Message> {
        let place = self.runs_where(&update.tree, update.real.as_deref());
        let lines = vec![
            format!(
                "Bring {} from {} into {}?",
                format::count(update.behind, "commit"),
                update.base,
                update.branch
            ),
            place,
        ];
        self.confirm(
            Confirm::change(
                "update-from-base",
                "Update from base",
                lines,
                "Update",
                Purpose::UpdateFromBase(update),
            )
            .undoable()
            // As `git pull` does there (pull.rebase); one click to change.
            .check("rebase instead of merge", rebase),
        )
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
        let mut lines = vec![format!("The branch {branch} merges into.")];
        if guessed && let Some(base) = &base {
            lines.push(format!(
                "{base} is a guess: nothing records where {branch} came from."
            ));
        }
        lines.push("Leave it empty to guess again.".into());
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
        let mut lines = vec![format!("Delete {}?", tilde(&path))];
        lines.push(if detached {
            "It is on no branch: commits made only there are lost.".into()
        } else {
            format!("The branch {name} stays.")
        });
        // Unknown: git's own check (it refuses with changes) stays.
        let dirty = changed.is_some_and(|n| n > 0);
        if let Some(n) = changed.filter(|n| *n > 0) {
            lines.push(format!(
                "{} there are lost too.",
                format::count(n as u64, "uncommitted change")
            ));
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

/// A merge or rebase waiting for its tree's tab
/// ([`App::integrate_in_tree`]).
#[derive(Debug, Clone)]
pub(super) struct IntegrateAfterOpen {
    pub path: PathBuf,
    pub what: Integrate,
}

/// What [`App::integrate_in_tree`] runs.
#[derive(Debug, Clone)]
pub(super) enum Integrate {
    /// Merge `branch` into the tree's branch (worktrees' `m`).
    Merge {
        branch: String,
        no_ff: bool,
        clean: Option<CleanAfterMerge>,
    },
    /// Replay the tree's branch onto `onto` (`u`, rebasing).
    Rebase { onto: String },
}

/// Bringing `base`'s `behind` new commits into `branch`, checked out at
/// `tree` (`real`: resolved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateFromBase {
    pub branch: String,
    pub base: String,
    pub tree: PathBuf,
    pub real: Option<PathBuf>,
    pub behind: u32,
}

/// The tree (and branch) to remove once its branch is merged into `base`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CleanAfterMerge {
    pub tree: PathBuf,
    pub branch: String,
    pub base: String,
}

impl App {
    /// Merges `branch` in the active tab; with `clean`, removes the merged
    /// tree and branch once the merge is done without conflicts.
    ///
    /// Then the merge is not recorded for ⌘Z: undoing it would leave the
    /// branch's commits on no branch.
    pub(super) fn merge_then_clean(
        &mut self,
        branch: String,
        no_ff: bool,
        clean: Option<CleanAfterMerge>,
    ) -> Task<Message> {
        let Some(clean) = clean else {
            return self.run_merge(branch, no_ff);
        };
        let Some(id) = self.active_tab().map(|t| t.repo.id()) else {
            return Task::none();
        };
        self.clean_after_merge = Some((id, clean));
        self.run_merge_recorded(branch, no_ff, false)
    }

    /// The merge in `id` ended: removes what [`Self::merge_then_clean`]
    /// set aside when it went through, else says how to do it later.
    pub(super) fn clean_after_merge(
        &mut self,
        id: RepoId,
        result: &Result<gf_git::Integration, String>,
    ) -> Task<Message> {
        let Some((_, clean)) = self.clean_after_merge.take_if(|(r, _)| *r == id) else {
            return Task::none();
        };
        match result {
            Ok(gf_git::Integration::Done) => {
                let CleanAfterMerge { tree, branch, base } = clean;
                let trees = vec![(tree.clone(), Some(branch.clone()))];
                self.op(
                    format!("remove worktree {} and branch {branch}", tilde(&tree)),
                    OpKind::RemoveWorktrees(vec![tree]),
                    move |r| async move {
                        super::dialogs::cleanup_worktrees(&r, &base, &trees, true).await
                    },
                )
            }
            Ok(gf_git::Integration::Conflicts(_)) => {
                self.log.note(
                    NoteKind::Info,
                    format!("{} was left as it is", tilde(&clean.tree)),
                    vec!["finish the merge, then remove it from the worktrees panel".into()],
                );
                Task::none()
            }
            Err(_) => Task::none(),
        }
    }
}

/// `cwd` (a tab's folder, resolved) is the tree at `path` (`real`: resolved).
fn same_folder(cwd: &Path, path: &Path, real: Option<&Path>) -> bool {
    cwd == path || real == Some(cwd)
}
