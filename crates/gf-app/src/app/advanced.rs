//! M4 operations in the app: merge / rebase / cherry-pick / revert (with
//! conflict handling), the in-progress banner, bisect, and undo.
//!
//! Every action here that changes the repository goes through the shared
//! confirmation (`App::confirm`), per the confirmation policy.

use std::future::Future;

use gf_core::model::{Commit, Oid, RepoId};
use gf_git::{BisectProgress, Integration, MergeStyle, Repository};
use iced::Task;

use super::dialogs::{Confirm, Purpose};
use super::{App, Message};
use crate::features::command_log::NoteKind;
use crate::features::diff_view::{self, DiffSource, Role};
use crate::features::history::{BatchAction, CommitAction};
use crate::features::op_banner;
use crate::repo_tab::UndoEntry;
use crate::widgets::format::truncate;

/// Undo entries kept per tab.
const MAX_UNDO: usize = 20;

impl App {
    /// Where HEAD is now, as an undo point for a step about to run.
    pub(super) fn undo_point(&self, label: &str, soft: bool) -> Option<UndoEntry> {
        let before = self
            .active_tab()?
            .snapshot
            .as_ref()?
            .status
            .head
            .oid
            .clone()?;
        Some(UndoEntry {
            label: label.to_owned(),
            before,
            soft,
        })
    }

    pub(super) fn push_undo(&mut self, id: RepoId, entry: Option<UndoEntry>) {
        if let (Some(entry), Some(tab)) = (entry, self.tab_mut(id)) {
            tab.undo.push(entry);
            if tab.undo.len() > MAX_UNDO {
                tab.undo.remove(0);
            }
        }
    }

    /// Runs merge / rebase / cherry-pick / revert / continue. Conflicts are
    /// reported as an outcome and lead the user to the working tree.
    pub(super) fn integrate_op<F, Fut>(
        &mut self,
        label: impl Into<String>,
        undoable: bool,
        f: F,
    ) -> Task<Message>
    where
        F: FnOnce(Repository) -> Fut,
        Fut: Future<Output = gf_core::Result<Integration>> + Send + 'static,
    {
        let label = label.into();
        let undo = if undoable {
            self.undo_point(&label, false)
        } else {
            None
        };
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        let repo = tab.repo.clone();
        let id = repo.id();
        let fut = f(repo);
        Task::perform(
            async move { fut.await.map_err(|e| e.to_string()) },
            move |result| Message::IntegrateDone {
                repo: id,
                label: label.clone(),
                result,
                undo: undo.clone(),
            },
        )
    }

    pub(super) fn on_integrate_done(
        &mut self,
        id: RepoId,
        label: String,
        result: Result<Integration, String>,
        undo: Option<UndoEntry>,
    ) -> Task<Message> {
        let refresh = self.request_refresh(id);
        match result {
            Ok(Integration::Done) => {
                self.flash(NoteKind::Info, format!("✓ {}", truncate(&label, 60)));
                self.push_undo(id, undo);
            }
            Ok(Integration::Conflicts(n)) => {
                // Resolving happens in the working tree view.
                if let Some(tab) = self.tab_mut(id) {
                    tab.code_view = Role::Local;
                }
                self.push_undo(id, undo);
                self.flash(
                    NoteKind::Error,
                    format!("{label}: {n} conflict(s) — resolve them, then continue (or abort)"),
                );
            }
            Err(e) => {
                self.log
                    .note(NoteKind::Error, format!("{label} failed"), vec![e]);
                self.flash(
                    NoteKind::Error,
                    format!("✗ {} — see log", truncate(&label, 40)),
                );
                self.settings.appearance.show_command_log = true;
            }
        }
        refresh
    }

    fn current_branch(&self) -> String {
        self.active_tab()
            .and_then(|t| t.snapshot.as_ref())
            .and_then(|s| s.status.head.branch.clone())
            .unwrap_or_else(|| "HEAD".into())
    }

    pub(super) fn ask_merge(&mut self, branch: String) -> Task<Message> {
        let current = self.current_branch();
        self.confirm(
            Confirm::change(
                "merge",
                "Merge",
                vec![
                    format!("Merge `{branch}` into `{current}`?"),
                    "Fast-forwards when possible; conflicts stop for you to resolve.".into(),
                ],
                "Merge",
                Purpose::Merge { branch },
            )
            .check("always create a merge commit (--no-ff)", false),
        )
    }

    pub(super) fn ask_rebase_onto(&mut self, onto: String) -> Task<Message> {
        let current = self.current_branch();
        self.confirm(Confirm::change(
            "rebase",
            "Rebase",
            vec![
                format!("Replay the commits of `{current}` on top of `{onto}`?"),
                "This rewrites those commits (new ids). If `{current}` was already"
                    .replace("{current}", &current),
                "pushed, publishing the result needs a force push.".into(),
            ],
            "Rebase",
            Purpose::RebaseOnto { onto },
        ))
    }

    pub(super) fn on_commit_action(&mut self, action: CommitAction, oid: Oid) -> Task<Message> {
        // The commit may come from a branch's or file's history, not only
        // the default one.
        let summary = self
            .active_tab()
            .and_then(|t| t.history.find(&oid, t.snapshot.as_deref()))
            .map(|c| format!(" \"{}\"", c.summary))
            .unwrap_or_default();
        let short = oid.short().to_owned();
        match action {
            CommitAction::CherryPick => {
                let current = self.current_branch();
                self.confirm(Confirm::change(
                    "cherry-pick",
                    "Cherry-pick",
                    vec![format!(
                        "Apply {short}{summary} on top of `{current}` as a new commit?"
                    )],
                    "Cherry-pick",
                    Purpose::CherryPick(vec![oid]),
                ))
            }
            CommitAction::Revert => self.confirm(Confirm::change(
                "revert",
                "Revert",
                vec![
                    format!("Add a new commit that undoes {short}{summary}?"),
                    "History is not rewritten; safe for pushed commits.".into(),
                ],
                "Revert",
                Purpose::Revert(vec![oid]),
            )),
            CommitAction::RebaseFrom => self.open_rebase_editor(oid),
            CommitAction::Reset => {
                let current = self.current_branch();
                let modes = super::dialogs::RESET_MODES
                    .iter()
                    .map(|(_, label, danger)| ((*label).to_owned(), *danger))
                    .collect();
                // Always a dialog (never skipped): the mode is chosen here.
                self.open_dialog(
                    crate::features::dialog::Dialog::confirm(
                        "Reset",
                        vec![
                            format!("Move `{current}` to {short}{summary}?"),
                            "Commits after it leave the branch; undo brings them back.".into(),
                        ],
                        "Reset",
                        Purpose::Reset {
                            to: oid,
                            mode: gf_git::ResetMode::Soft,
                        },
                    )
                    .severity(crate::features::dialog::Severity::Change)
                    .choice("what happens to their changes:", modes, 0),
                )
            }
            CommitAction::BisectFrom => self.confirm(Confirm::change(
                "bisect",
                "Start bisect",
                vec![
                    format!("Search between {short} (good) and HEAD (bad)?"),
                    "gf checks out commits one by one; mark each good or bad in the".into(),
                    "banner until the first bad commit is found.".into(),
                ],
                "Start",
                Purpose::BisectStart { good: oid },
            )),
        }
    }

    pub(super) fn run_merge(&mut self, branch: String, no_ff: bool) -> Task<Message> {
        let style = if no_ff {
            MergeStyle::NoFastForward
        } else {
            MergeStyle::Default
        };
        self.integrate_op(format!("merge {branch}"), true, move |r| async move {
            r.merge(&branch, style).await
        })
    }

    pub(super) fn run_rebase_onto(&mut self, onto: String) -> Task<Message> {
        self.integrate_op(format!("rebase onto {onto}"), true, move |r| async move {
            r.rebase(&onto).await
        })
    }

    /// Cherry-picks `oids` in the given order (pass oldest first).
    pub(super) fn run_cherry_pick(&mut self, oids: Vec<Oid>) -> Task<Message> {
        let revs: Vec<String> = oids.iter().map(Oid::to_string).collect();
        self.integrate_op(
            format!("cherry-pick {}", short_list(&oids)),
            true,
            move |r| async move { r.cherry_pick(&revs).await },
        )
    }

    /// Reverts `oids` in the given order (pass newest first).
    pub(super) fn run_revert(&mut self, oids: Vec<Oid>) -> Task<Message> {
        let revs: Vec<String> = oids.iter().map(Oid::to_string).collect();
        self.integrate_op(
            format!("revert {}", short_list(&oids)),
            true,
            move |r| async move { r.revert_many(&revs).await },
        )
    }

    // ------------------------------------------------- several commits

    /// Combined diff of the selected commits (newest first).
    pub(super) fn show_range(&mut self, commits: Vec<Commit>) -> Task<Message> {
        let (Some(newest), Some(oldest)) = (commits.first(), commits.last()) else {
            return Task::none();
        };
        let from = oldest
            .parents
            .first()
            .map_or_else(|| diff_view::EMPTY_TREE.to_owned(), Oid::to_string);
        self.show_diff(DiffSource::Range {
            from,
            to: newest.id.clone(),
            count: commits.len(),
            contiguous: is_chain(&commits),
        })
    }

    pub(super) fn on_batch(&mut self, action: BatchAction, commits: Vec<Commit>) -> Task<Message> {
        let n = commits.len();
        let newest_first: Vec<Oid> = commits.iter().map(|c| c.id.clone()).collect();
        let oldest_first: Vec<Oid> = newest_first.iter().rev().cloned().collect();
        let lines = |verb: &str| -> Vec<String> {
            let mut v: Vec<String> = commits
                .iter()
                .take(8)
                .map(|c| format!("  {} {}", c.id.short(), c.summary))
                .collect();
            if n > 8 {
                v.push(format!("  … and {} more", n - 8));
            }
            v.insert(0, format!("{verb} these {n} commits?"));
            v
        };
        match action {
            BatchAction::CherryPick => {
                let current = self.current_branch();
                let mut body = lines(&format!("Apply on top of `{current}`, oldest first,"));
                body.push("Stops on the first conflict; continue or abort from the banner.".into());
                self.confirm(Confirm::change(
                    "cherry-pick",
                    "Cherry-pick commits",
                    body,
                    format!("Cherry-pick {n}"),
                    Purpose::CherryPick(oldest_first),
                ))
            }
            BatchAction::Revert => {
                let mut body = lines("Add commits that undo, newest first,");
                body.push("History is not rewritten; safe for pushed commits.".into());
                self.confirm(Confirm::change(
                    "revert",
                    "Revert commits",
                    body,
                    format!("Revert {n}"),
                    Purpose::Revert(newest_first),
                ))
            }
            BatchAction::Squash => {
                if !is_chain(&commits) {
                    self.flash(
                        NoteKind::Error,
                        "squash needs a continuous range: shift-click to select one",
                    );
                    return Task::none();
                }
                let Some(base) = commits.last().and_then(|c| c.parents.first()).cloned() else {
                    self.flash(NoteKind::Error, "cannot squash the root commit here");
                    return Task::none();
                };
                // The editor shows the plan and the combined message; nothing
                // is rewritten until it is started (with its own confirm).
                self.load_rebase_plan(base, Some(newest_first))
            }
            BatchAction::BranchAtNewest => {
                self.new_branch_dialog(Some(newest_first[0].to_string()))
            }
            BatchAction::CopyIds => {
                self.flash(NoteKind::Info, format!("copied {n} commit ids"));
                let text = newest_first
                    .iter()
                    .map(Oid::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                iced::clipboard::write(text)
            }
            BatchAction::Clear => Task::none(),
        }
    }

    // ------------------------------------------------------------- banner

    pub(super) fn on_banner(&mut self, m: op_banner::Message) -> Task<Message> {
        match m {
            op_banner::Message::Continue => self.integrate_op("continue", false, |r| async move {
                r.continue_operation().await
            }),
            op_banner::Message::Skip => self.confirm(Confirm::delete(
                "Skip commit",
                vec![
                    "Leave out the commit that stopped here?".into(),
                    "Its changes will not be in the result.".into(),
                ],
                "Skip",
                Purpose::SkipOperation,
            )),
            op_banner::Message::Abort => self.confirm(Confirm::delete(
                "Abort",
                vec![
                    "Stop and return to how things were before it started?".into(),
                    "Conflict resolutions made so far are thrown away.".into(),
                ],
                "Abort",
                Purpose::AbortOperation,
            )),
            op_banner::Message::ShowConflicts => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.code_view = Role::Local;
                }
                Task::none()
            }
            op_banner::Message::Bisect(v) => {
                self.bisect_step(move |r| async move { r.bisect_mark(v).await })
            }
            op_banner::Message::BisectReset => self.confirm(Confirm::change(
                "bisect-reset",
                "End bisect",
                vec!["Stop bisecting and go back to the branch you started from?".into()],
                "End",
                Purpose::BisectReset,
            )),
        }
    }

    pub(super) fn bisect_step<F, Fut>(&mut self, f: F) -> Task<Message>
    where
        F: FnOnce(Repository) -> Fut,
        Fut: Future<Output = gf_core::Result<BisectProgress>> + Send + 'static,
    {
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        let repo = tab.repo.clone();
        let id = repo.id();
        let fut = f(repo);
        Task::perform(
            async move { fut.await.map_err(|e| e.to_string()) },
            move |result| Message::BisectDone(id, result),
        )
    }

    pub(super) fn on_bisect_done(
        &mut self,
        id: RepoId,
        result: Result<BisectProgress, String>,
    ) -> Task<Message> {
        match result {
            Ok(p) => {
                if let Some(first) = &p.first_bad {
                    self.flash(
                        NoteKind::Info,
                        format!(
                            "found it: {} is the first bad commit — end bisect to go back",
                            first.short()
                        ),
                    );
                }
                if let Some(tab) = self.tab_mut(id) {
                    tab.bisect = Some(p);
                }
            }
            Err(e) => {
                self.log.note(NoteKind::Error, "bisect failed", vec![e]);
                self.flash(NoteKind::Error, "✗ bisect — see log");
            }
        }
        self.request_refresh(id)
    }

    // --------------------------------------------------------------- undo

    pub(super) fn ask_undo(&mut self) -> Task<Message> {
        let Some(entry) = self.active_tab().and_then(|t| t.undo.last().cloned()) else {
            self.flash(NoteKind::Info, "nothing to undo");
            return Task::none();
        };
        let branch = self.current_branch();
        let keeps = if entry.soft {
            "its changes stay staged so nothing is lost"
        } else {
            "files are restored; uncommitted changes are kept (git refuses if they conflict)"
        };
        self.confirm(Confirm::change(
            "undo",
            "Undo",
            vec![
                format!("Undo \"{}\"?", entry.label),
                format!(
                    "`{branch}` moves back to {} — {keeps}.",
                    entry.before.short()
                ),
            ],
            "Undo",
            Purpose::Undo(entry),
        ))
    }

    pub(super) fn run_undo(&mut self, entry: UndoEntry) -> Task<Message> {
        // The entry is removed when the undo succeeds (see `on_op_done`).
        let label = format!("undo {}", entry.label);
        self.op(label, super::OpKind::Undo, move |r| async move {
            r.undo_to(&entry.before, entry.soft).await
        })
    }

    // ---------------------------------------------------- interactive rebase

    pub(super) fn open_rebase_editor(&mut self, base: Oid) -> Task<Message> {
        self.load_rebase_plan(base, None)
    }

    fn load_rebase_plan(&mut self, base: Oid, squash: Option<Vec<Oid>>) -> Task<Message> {
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        let repo = tab.repo.clone();
        Task::perform(
            {
                let base = base.clone();
                async move {
                    repo.rebase_plan(Some(&base))
                        .await
                        .map_err(|e| e.to_string())
                }
            },
            move |plan| Message::RebasePlanLoaded(base.clone(), plan, squash.clone()),
        )
    }

    pub(super) fn on_rebase_plan(
        &mut self,
        base: Oid,
        plan: Result<Vec<gf_git::TodoItem>, String>,
        squash: Option<Vec<Oid>>,
    ) -> Task<Message> {
        match plan {
            Ok(items) if items.is_empty() => {
                self.flash(
                    NoteKind::Info,
                    "no commits after this one on the current branch",
                );
                Task::none()
            }
            Ok(items) => {
                let editor = crate::features::rebase_editor::State::new(base, items);
                match squash {
                    None => self.rebase_editor = Some(editor),
                    Some(ids) => match editor.squash(&ids) {
                        Ok(e) => self.rebase_editor = Some(e),
                        Err(e) => self.flash(NoteKind::Error, e),
                    },
                }
                Task::none()
            }
            Err(e) => {
                self.log
                    .note(NoteKind::Error, "interactive rebase", vec![e]);
                Task::none()
            }
        }
    }

    pub(super) fn on_rebase_editor(
        &mut self,
        m: crate::features::rebase_editor::Message,
    ) -> Task<Message> {
        use crate::features::rebase_editor::Output;
        let Some(editor) = self.rebase_editor.as_mut() else {
            return Task::none();
        };
        match editor.update(m) {
            Some(Output::Cancel) => {
                self.rebase_editor = None;
                Task::none()
            }
            Some(Output::Start { base, items }) => {
                self.rebase_editor = None;
                let n = items.len();
                self.confirm(Confirm::change(
                    "rebase-interactive",
                    "Rewrite history",
                    vec![
                        format!("Rewrite {n} commit(s) after {} as planned?", base.short()),
                        "Commit ids change. If they were pushed, publishing needs a force push."
                            .into(),
                        "Undo is available afterwards.".into(),
                    ],
                    "Rewrite",
                    Purpose::RebaseInteractive { base, items },
                ))
            }
            None => Task::none(),
        }
    }

    pub(super) fn run_rebase_interactive(
        &mut self,
        base: Oid,
        items: Vec<gf_git::TodoItem>,
    ) -> Task<Message> {
        self.integrate_op(
            format!("interactive rebase onto {}", base.short()),
            true,
            move |r| async move { r.rebase_interactive(Some(&base), &items).await },
        )
    }

    // ------------------------------------------------------------ conflicts

    pub(super) fn open_conflict(&mut self, path: std::path::PathBuf) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        tab.code_view = Role::Local;
        tab.conflict = Some(crate::features::conflict_view::State::loading(path.clone()));
        let repo = tab.repo.clone();
        let id = repo.id();
        Task::perform(
            {
                let path = path.clone();
                async move {
                    repo.conflict_segments(&path)
                        .await
                        .map_err(|e| e.to_string())
                }
            },
            move |result| Message::ConflictLoaded(id, path.clone(), result),
        )
    }

    pub(super) fn on_conflict_message(
        &mut self,
        m: crate::features::conflict_view::Message,
    ) -> Task<Message> {
        use crate::features::conflict_view::Output;
        let Some(out) = self
            .active_tab_mut()
            .and_then(|t| t.conflict.as_mut())
            .and_then(|c| c.update(m))
        else {
            return Task::none();
        };
        match out {
            Output::Close => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.conflict = None;
                }
                Task::none()
            }
            Output::Apply {
                path,
                segments,
                choices,
            } => {
                let n = choices.iter().filter(|c| c.is_some()).count();
                let left = choices.len() - n;
                let tail = if left == 0 {
                    "Every conflict is chosen: the file is written and staged.".to_owned()
                } else {
                    format!("{left} unchosen conflict(s) keep their markers.")
                };
                self.confirm(Confirm::change(
                    "resolve",
                    "Resolve conflicts",
                    vec![
                        format!("Write {} with {n} chosen block(s)?", path.display()),
                        tail,
                    ],
                    "Write",
                    Purpose::ResolveBlocks {
                        path,
                        segments,
                        choices,
                    },
                ))
            }
            Output::TakeFile(path, side) => {
                let (keep, lose) = match side {
                    gf_git::Side::Ours => ("your version", "incoming"),
                    gf_git::Side::Theirs => ("the incoming version", "your"),
                };
                self.confirm(Confirm::change(
                    "resolve",
                    "Take whole file",
                    vec![
                        format!("Keep {keep} of {} and stage it?", path.display()),
                        format!("All {lose} changes to this file are left out."),
                    ],
                    "Take",
                    Purpose::TakeSide(path, side),
                ))
            }
            Output::MarkResolved(path) => self.confirm(Confirm::change(
                "resolve",
                "Mark resolved",
                vec![
                    format!("Stage {} as it is now?", path.display()),
                    "Make sure no conflict markers are left in it.".into(),
                ],
                "Stage",
                Purpose::MarkResolved(path),
            )),
        }
    }

    pub(super) fn run_resolve(
        &mut self,
        path: std::path::PathBuf,
        segments: Vec<gf_git::conflict::Segment>,
        choices: Vec<Option<gf_git::conflict::Choice>>,
    ) -> Task<Message> {
        // The resolver reloads after the refresh this triggers (see
        // `on_loaded`), and closes once the file is no longer conflicted.
        self.op(
            format!("resolve {}", path.display()),
            super::OpKind::Generic,
            move |r| async move {
                r.resolve_blocks(&path, &segments, &choices)
                    .await
                    .map(|_| ())
            },
        )
    }

    // ------------------------------------------------- blame / file history

    pub(super) fn open_blame(
        &mut self,
        path: std::path::PathBuf,
        rev: Option<String>,
    ) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        tab.code_view = Role::Commit;
        tab.blame = Some(crate::features::blame_view::State::loading(
            path.clone(),
            rev.clone(),
        ));
        let repo = tab.repo.clone();
        let id = repo.id();
        Task::perform(
            {
                let path = path.clone();
                async move {
                    repo.blame(&path, rev.as_deref())
                        .await
                        .map_err(|e| e.to_string())
                }
            },
            move |result| Message::BlameLoaded(id, path.clone(), result),
        )
    }

    pub(super) fn on_blame_message(
        &mut self,
        m: crate::features::blame_view::Message,
    ) -> Task<Message> {
        use crate::features::blame_view::Output;
        let Some(out) = self
            .active_tab_mut()
            .and_then(|t| t.blame.as_mut())
            .and_then(|b| b.update(m))
        else {
            return Task::none();
        };
        if let Some(tab) = self.active_tab_mut() {
            tab.blame = None;
        }
        match out {
            Output::Close => Task::none(),
            Output::ShowCommit(oid) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.history.select(oid.clone());
                }
                self.show_diff(crate::features::diff_view::DiffSource::Commit(oid))
            }
        }
    }

    pub(super) fn open_file_history(&mut self, path: std::path::PathBuf) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        tab.code_view = Role::Commit;
        tab.sidebar.clear_selection();
        let key = format!("file: {}", path.display());
        tab.history.set_revision(Some(key.clone()));
        super::ops::load_history(tab, key)
    }
}

/// `abc1234, def5678` (at most three, then `+N`).
fn short_list(oids: &[Oid]) -> String {
    let mut s = oids
        .iter()
        .take(3)
        .map(|o| o.short().to_owned())
        .collect::<Vec<_>>()
        .join(", ");
    if oids.len() > 3 {
        s.push_str(&format!(" +{}", oids.len() - 3));
    }
    s
}

/// Whether `commits` (newest first) are one first-parent chain, so the diff
/// from the oldest's parent to the newest holds exactly their changes.
fn is_chain(commits: &[Commit]) -> bool {
    commits
        .windows(2)
        .all(|w| w[0].parents.first() == Some(&w[1].id))
}
