//! M4 operations in the app: merge / rebase / cherry-pick / revert (with
//! conflict handling), the in-progress banner and bisect.
//!
//! Every action here that changes the repository goes through the shared
//! confirmation (`App::confirm`), per the confirmation policy.

use std::future::Future;

use gf_core::model::{Commit, Oid, RepoId};
use gf_git::{BisectProgress, HeadMode, Integration, MergeStyle, Repository, Scope};
use iced::Task;

use super::dialogs::{Confirm, Purpose};
use super::{App, Message};
use crate::features::command_log::NoteKind;
use crate::features::diff_view::{self, DiffSource, Role};
use crate::features::history::{BatchAction, CommitAction};
use crate::features::op_banner;
use crate::widgets::format::truncate;

/// What an integration recorded for undo: a finished step, or the state
/// before it when it stopped on conflicts (recorded once continued).
#[derive(Debug, Clone)]
pub enum Recorded {
    Step(Box<crate::undo::Step>),
    Pending(Box<crate::undo::Pending>),
}

impl App {
    /// Runs merge / rebase / cherry-pick / revert / continue. Conflicts are
    /// reported as an outcome and lead the user to the working tree.
    /// `undoable` starts an undo step; continuing one that stopped on
    /// conflicts finishes the step it started.
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
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        let pending = if undoable { None } else { tab.pending.clone() };
        let repo = tab.repo.clone();
        let id = repo.id();
        let fut = f(repo.clone());
        let step_label = label.clone();
        Task::perform(
            async move {
                let scope = Scope::head();
                let started = if undoable {
                    repo.capture(&scope)
                        .await
                        .ok()
                        .map(|before| crate::undo::Pending {
                            label: step_label,
                            scope,
                            before,
                            mode: HeadMode::Keep,
                        })
                } else {
                    pending
                };
                let result = fut.await.map_err(|e| e.to_string());
                let record = match (&result, started) {
                    (Ok(Integration::Done), Some(p)) => repo
                        .capture(&p.scope)
                        .await
                        .ok()
                        .filter(|a| *a != p.before)
                        .map(|after| {
                            Box::new(Recorded::Step(Box::new(crate::undo::Step::new(
                                p.label, p.scope, p.before, after, p.mode,
                            ))))
                        }),
                    (Ok(Integration::Conflicts(_)), Some(p)) => {
                        Some(Box::new(Recorded::Pending(Box::new(p))))
                    }
                    _ => None,
                };
                (result, record)
            },
            move |(result, record)| Message::IntegrateDone {
                repo: id,
                label: label.clone(),
                result,
                record,
            },
        )
    }

    pub(super) fn on_integrate_done(
        &mut self,
        id: RepoId,
        label: String,
        result: Result<Integration, String>,
        record: Option<Box<Recorded>>,
    ) -> Task<Message> {
        let refresh = self.request_refresh(id);
        let recorded = matches!(record.as_deref(), Some(Recorded::Step(_)));
        match record.map(|r| *r) {
            Some(Recorded::Step(step)) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.pending = None;
                }
                self.record_step(id, *step);
            }
            Some(Recorded::Pending(p)) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.pending = Some(*p);
                }
            }
            None => {}
        }
        match result {
            Ok(Integration::Done) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.pending = None;
                }
                let hint = if recorded { " · ⌘Z undo" } else { "" };
                self.flash(NoteKind::Info, format!("✓ {}{hint}", truncate(&label, 60)));
            }
            Ok(Integration::Conflicts(n)) => {
                // Resolving happens in the working tree view.
                if let Some(tab) = self.tab_mut(id) {
                    tab.code_view = Role::Local;
                }
                self.flash(
                    NoteKind::Error,
                    format!("{label}: {n} conflict(s) — resolve them, then continue (or abort)"),
                );
            }
            Err(e) => {
                self.log
                    .note(NoteKind::Error, format!("{label} failed"), vec![e.clone()]);
                self.flash(
                    NoteKind::Error,
                    format!("✗ {} — see log", truncate(&label, 40)),
                );
                self.settings.appearance.show_command_log = true;
                return Task::batch([refresh, self.show_failure(&label, &e)]);
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
                    .choice("what happens to their changes:", modes, 0)
                    .undoable(),
                )
            }
            CommitAction::SquashIntoPrevious => {
                let parent = self
                    .active_tab()
                    .and_then(|t| t.history.find(&oid, t.snapshot.as_deref()))
                    .and_then(|c| (c.parents.len() == 1).then(|| c.parents[0].clone()));
                match parent {
                    Some(parent) => self.ask_squash(vec![oid, parent], 2),
                    None => {
                        self.flash(
                            NoteKind::Error,
                            "nothing to squash into: this is the first commit or a merge",
                        );
                        Task::none()
                    }
                }
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
            BatchAction::Squash => match self.fill_gaps(&commits) {
                Ok(ids) => self.ask_squash(ids, n),
                Err(e) => {
                    self.flash(NoteKind::Error, e);
                    Task::none()
                }
            },
            BatchAction::SquashInEditor => {
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
            // Same as popping it from the sidebar (same confirmation).
            op_banner::Message::PopLeft(i) => {
                self.on_sidebar(crate::features::sidebar::Output::StashPop(i))
            }
            op_banner::Message::DismissLeft(id) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.left_dismissed.push(id);
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
                self.log
                    .note(NoteKind::Error, "bisect failed", vec![e.clone()]);
                self.flash(NoteKind::Error, "✗ bisect — see log");
                let report = self.show_failure("bisect", &e);
                return Task::batch([self.request_refresh(id), report]);
            }
        }
        self.request_refresh(id)
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

    // --------------------------------------------------------------- squash

    /// The selected commits (list order, newest first) plus any commits
    /// between them on the first-parent line, so a ⌘-click selection with
    /// gaps still squashes one continuous range.
    fn fill_gaps(&self, commits: &[Commit]) -> Result<Vec<Oid>, String> {
        if is_chain(commits) {
            return Ok(commits.iter().map(|c| c.id.clone()).collect());
        }
        let tab = self.active_tab().ok_or("no repository open")?;
        let snap = tab.snapshot.as_deref();
        let wanted: Vec<&Oid> = commits.iter().map(|c| &c.id).collect();
        let not_a_line = || {
            "the selected commits are not on one line of history — select a continuous range"
                .to_owned()
        };
        let mut ids = Vec::new();
        let mut current = commits.first().ok_or_else(not_a_line)?.clone();
        loop {
            ids.push(current.id.clone());
            if wanted.iter().all(|w| ids.contains(w)) {
                return Ok(ids);
            }
            if ids.len() > MAX_SQUASH_GAP + wanted.len() {
                return Err(not_a_line());
            }
            current = current
                .parents
                .first()
                .and_then(|p| tab.history.find(p, snap))
                .ok_or_else(not_a_line)?
                .clone();
        }
    }

    /// Checks the range in the background, then asks with the squash
    /// dialog (see [`Self::on_squash_preview`]).
    fn ask_squash(&mut self, ids: Vec<Oid>, selected: usize) -> Task<Message> {
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        let repo = tab.repo.clone();
        Task::perform(
            {
                let ids = ids.clone();
                async move { repo.squash_preview(&ids).await.map_err(|e| e.to_string()) }
            },
            move |result| Message::SquashPreviewed {
                ids: ids.clone(),
                selected,
                result,
            },
        )
    }

    pub(super) fn on_squash_preview(
        &mut self,
        ids: Vec<Oid>,
        selected: usize,
        result: Result<gf_git::SquashPreview, String>,
    ) -> Task<Message> {
        let preview = match result {
            Ok(p) => p,
            Err(e) => {
                self.flash(NoteKind::Error, format!("cannot squash: {e}"));
                return Task::none();
            }
        };
        let dirty = self
            .active_tab()
            .and_then(|t| t.snapshot.as_deref())
            .is_some_and(|s| {
                s.status
                    .entries
                    .iter()
                    .any(|e| !matches!(e, gf_core::model::StatusEntry::Untracked { .. }))
            });
        let (summary, details) = squash_message(&preview.commits);
        let dialog = squash_dialog(&preview, selected, dirty, &summary, &details, ids);
        self.open_dialog(dialog)
    }

    pub(super) fn run_squash(
        &mut self,
        ids: Vec<Oid>,
        message: String,
        autostash: bool,
    ) -> Task<Message> {
        let n = ids.len();
        self.integrate_op(format!("squash {n} commits"), true, move |r| async move {
            r.squash(&ids, &message, autostash).await
        })
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

    /// Resolves a conflicted file with one side, whole (asks first).
    pub(super) fn ask_take_side(
        &mut self,
        path: std::path::PathBuf,
        side: gf_git::Side,
    ) -> Task<Message> {
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
            Output::TakeFile(path, side) => self.ask_take_side(path, side),
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
        self.op_recorded(
            format!("resolve {}", path.display()),
            super::OpKind::Generic,
            Some(super::ops::Record::new(crate::undo::resolve(path.clone()))),
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
        match out {
            Output::Close => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.blame = None;
                }
                Task::none()
            }
            // Blame stays open: the history finds the commit beside it.
            Output::ShowCommit(oid) => self.reveal_commit(oid),
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
/// Commits `fill_gaps` may add between the selected ones.
const MAX_SQUASH_GAP: usize = 200;

/// `summary` alone, or `summary`, a blank line and `details`.
pub(super) fn commit_message(summary: &str, details: &str) -> String {
    let (summary, details) = (summary.trim(), details.trim_end());
    if details.trim().is_empty() {
        summary.to_owned()
    } else {
        format!("{summary}\n\n{details}")
    }
}

/// Suggested message for melding `commits` (oldest first): the oldest's
/// summary, then its body and a `- summary` line (with the body
/// indented) for each later commit.
fn squash_message(commits: &[Commit]) -> (String, String) {
    let Some((oldest, rest)) = commits.split_first() else {
        return (String::new(), String::new());
    };
    let mut details = Vec::new();
    if !oldest.body.trim().is_empty() {
        details.push(oldest.body.trim_end().to_owned());
        details.push(String::new());
    }
    for c in rest {
        details.push(format!("- {}", c.summary));
        details.extend(
            c.body
                .trim_end()
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| format!("  {l}")),
        );
    }
    (
        oldest.summary.clone(),
        details.join("\n").trim_end().to_owned(),
    )
}

fn squash_dialog(
    preview: &gf_git::SquashPreview,
    selected: usize,
    dirty: bool,
    summary: &str,
    details: &str,
    ids: Vec<Oid>,
) -> crate::features::dialog::Dialog<Purpose> {
    use crate::features::dialog::{Dialog, Severity};
    let n = preview.commits.len();
    let mut body = vec![format!("Meld these {n} commits (oldest first) into one:")];
    body.extend(
        preview
            .commits
            .iter()
            .take(8)
            .map(|c| format!("  {} {}", c.id.short(), truncate(&c.summary, 60))),
    );
    if n > 8 {
        body.push(format!("  … and {} more", n - 8));
    }
    if n > selected {
        body.push(format!(
            "Includes {} commit(s) between the ones you selected.",
            n - selected
        ));
    }
    if preview.base.is_none() {
        body.push("Starts at the first commit of the repository.".into());
    }
    if preview.pushed > 0 {
        body.push(format!(
            "⚠ {} of them are already on a remote: publishing needs a force push.",
            preview.pushed
        ));
    }
    body.push("The oldest commit's author and date are kept; undo restores the commits.".into());
    let mut dialog = Dialog::confirm(
        format!("Squash {n} commits"),
        body,
        "Squash",
        Purpose::Squash { ids },
    )
    .severity(Severity::Change)
    .field(
        "summary",
        summary,
        "one line: what the combined commit does",
    )
    .area("details", details, "details (optional)");
    if dirty {
        dialog = dialog.check(
            "stash my uncommitted changes and restore them afterwards",
            true,
        );
    }
    dialog
}

fn is_chain(commits: &[Commit]) -> bool {
    commits
        .windows(2)
        .all(|w| w[0].parents.first() == Some(&w[1].id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gf_core::model::{Signature, Timestamp};

    fn commit(n: u8, parent: Option<u8>, summary: &str, body: &str) -> Commit {
        let sig = Signature {
            name: String::new(),
            email: String::new(),
            time: Timestamp {
                seconds: 0,
                offset_minutes: 0,
            },
        };
        let id = |n: u8| -> Oid { format!("{n:040x}").parse().expect("oid") };
        Commit {
            id: id(n),
            parents: parent.map(id).into_iter().collect(),
            author: sig.clone(),
            committer: sig,
            summary: summary.into(),
            body: body.into(),
        }
    }

    #[test]
    fn squash_message_keeps_the_oldest_summary_and_lists_the_rest() {
        let commits = [
            commit(1, None, "feat: login", "Adds the form."),
            commit(2, Some(1), "fix typo", ""),
            commit(3, Some(2), "wip", "half done\n\nmore"),
        ];
        let (summary, details) = squash_message(&commits);
        assert_eq!(summary, "feat: login");
        assert_eq!(
            details,
            "Adds the form.\n\n- fix typo\n- wip\n  half done\n  more"
        );
        assert_eq!(
            squash_message(&commits[1..2]),
            ("fix typo".into(), String::new())
        );
    }

    #[test]
    fn commit_message_joins_summary_and_details() {
        assert_eq!(commit_message(" a ", ""), "a");
        assert_eq!(commit_message("a", "  \n"), "a");
        assert_eq!(commit_message("a", "b\nc\n"), "a\n\nb\nc");
    }

    #[test]
    fn squash_dialog_warns_and_offers_the_stash() {
        let preview = gf_git::SquashPreview {
            base: None,
            commits: vec![commit(1, None, "a", ""), commit(2, Some(1), "b", "")],
            pushed: 1,
        };
        let ids = preview.commits.iter().map(|c| c.id.clone()).collect();
        let d = squash_dialog(&preview, 1, true, "a", "- b", ids);
        let body = d.body.join("\n");
        assert!(body.contains("Includes 1 commit(s) between"));
        assert!(body.contains("first commit of the repository"));
        assert!(body.contains("force push"));
        assert_eq!(d.fields[0].value, "a");
        assert_eq!(d.checks.len(), 1, "stash offered for a dirty tree");

        let d = squash_dialog(&preview, 2, false, "a", "", Vec::new());
        assert!(d.checks.is_empty());
        assert!(!d.body.join("\n").contains("between"));
    }

    #[test]
    fn chains_follow_first_parents() {
        let a = commit(1, None, "a", "");
        let b = commit(2, Some(1), "b", "");
        let c = commit(3, Some(2), "c", "");
        assert!(is_chain(&[c.clone(), b, a.clone()]));
        assert!(!is_chain(&[c, a]));
    }
}
