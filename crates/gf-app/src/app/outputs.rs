//! Reactions to feature outputs: each feature asks, the app decides what
//! git work to run (directly, or after a confirmation dialog).

use gf_core::model::{Branch, RefKind};
use gf_git::{CommitOptions, FileVersion, PullMode, StashOptions};
use iced::Task;
use iced::widget::operation;

use super::dialogs::{Confirm, Purpose};
use super::ops::Record;
use super::ops::{load_diff, load_history};
use super::{App, Message, OpKind, Shortcut};
use crate::feature::Feature;
use crate::features::command_log::NoteKind;
use crate::features::diff_view::{self, Action, DiffSource};
use crate::features::prompt::PROMPT_ID;
use crate::features::prompt::parse::{BUILTINS, PromptCommand};
use crate::features::{changes, code_view, commit_box, history, sidebar, toolbar};
use crate::repo_tab::{Parts, Snapshot};
use crate::theme;
use crate::undo;

impl App {
    pub(super) fn show_diff(&mut self, source: DiffSource) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        // Picking a file or commit brings its view to the front.
        let role = source.role();
        tab.code_view = role;
        // Repeated clicks on the same commit must not re-run git.
        if tab.diff(role).is_current(&source) {
            return Task::none();
        }
        load_diff(tab, source)
    }

    pub(super) fn open_code_view(
        &mut self,
        path: std::path::PathBuf,
        version: FileVersion,
    ) -> Task<Message> {
        let Some(repo) = self.active_tab().map(|tab| tab.repo.clone()) else {
            return Task::none();
        };
        self.code_dialog_request = self.code_dialog_request.wrapping_add(1);
        let (viewer, load) = code_view::State::open(self.code_dialog_request, repo, path, version);
        self.code_dialog = Some(viewer);
        Task::batch([
            load.map(Message::CodeDialog),
            iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus()),
        ])
    }

    pub(super) fn on_code_view(&mut self, m: code_view::Message) -> Task<Message> {
        let Some(viewer) = self.code_dialog.as_mut() else {
            return Task::none();
        };
        let m = match m {
            code_view::Message::Pressed(line, pointer)
                if self.modifiers.shift() && pointer.button == iced::mouse::Button::Left =>
            {
                code_view::Message::Extend(line, pointer)
            }
            other => other,
        };
        let update = viewer.update(m);
        match update.output {
            Some(code_view::Output::Close) => {
                self.code_dialog = None;
                self.restore_focus()
            }
            Some(code_view::Output::Copy(text)) => self.copy_text(text, "code"),
            Some(code_view::Output::Menu(menu)) => self.open_menu(menu.map(Message::CodeDialog)),
            None => update.task.map(Message::CodeDialog),
        }
    }

    pub(super) fn on_changes(&mut self, out: changes::Output) -> Task<Message> {
        match out {
            changes::Output::ShowDiff(source) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.conflict = None;
                }
                self.show_diff(source)
            }
            changes::Output::ViewFile(path, version) => self.open_code_view(path, version),
            changes::Output::ResolveConflict(path) => self.open_conflict(path),
            changes::Output::Stage(paths) => {
                let label = format!("stage {}", names(&paths));
                let record = Record::new(undo::index(paths.clone()));
                self.op_recorded(label, OpKind::Generic, Some(record), move |r| async move {
                    r.stage(&paths).await
                })
            }
            changes::Output::Unstage(paths) => {
                let label = format!("unstage {}", names(&paths));
                let record = Record::new(undo::index(paths.clone()));
                self.op_recorded(label, OpKind::Generic, Some(record), move |r| async move {
                    r.unstage(&paths).await
                })
            }
            changes::Output::StageAll => {
                let record = Record::new(undo::index(self.changed_paths()));
                self.op_recorded("stage all", OpKind::Generic, Some(record), |r| async move {
                    r.stage_all().await
                })
            }
            changes::Output::UnstageAll => {
                let record = Record::new(undo::index(self.changed_paths()));
                self.op_recorded(
                    "unstage all",
                    OpKind::Generic,
                    Some(record),
                    |r| async move { r.unstage_all().await },
                )
            }
            changes::Output::Discard { tracked, untracked } => {
                self.confirm_discard(tracked, untracked)
            }
            changes::Output::Menu { show, menu } => {
                // A right-clicked file shows its diff like a click.
                let shown = show.map_or_else(Task::none, |s| {
                    self.on_changes(changes::Output::ShowDiff(s))
                });
                let open = self.open_menu(menu.map(Message::Changes));
                Task::batch([shown, open])
            }
            changes::Output::FileHistory(path) => self.open_file_history(path),
            changes::Output::Blame(path) => self.open_blame(path, None),
            changes::Output::CopyPath(path) => {
                let text = path.display().to_string();
                self.flash(NoteKind::Info, format!("copied {text}"));
                iced::clipboard::write(text)
            }
            changes::Output::Reveal(path) => {
                if let Some(abs) = self.active_tab().map(|t| t.repo.cwd().join(&path))
                    && let Err(e) = reveal(&abs)
                {
                    self.flash(NoteKind::Error, format!("could not show it: {e}"));
                }
                Task::none()
            }
            changes::Output::StashFile(path) => {
                let label = format!("stash {}", path.display());
                let record = Record::new(undo::stash(vec![path.clone()]));
                let opts = StashOptions {
                    message: None,
                    include_untracked: false,
                    keep_index: false,
                    paths: vec![path],
                };
                self.op_recorded(label, OpKind::Generic, Some(record), move |r| async move {
                    r.stash_push(&opts).await
                })
            }
            changes::Output::Ignore(pattern) => {
                let label = format!("ignore {pattern}");
                let record = Record::new(undo::files(vec![".gitignore".into()]));
                self.op_recorded(label, OpKind::Generic, Some(record), move |r| async move {
                    r.ignore(&pattern).await
                })
            }
            changes::Output::TakeSide(path, side) => self.ask_take_side(path, side),
        }
    }

    /// Asks before discarding work tree changes. Submodules are named
    /// apart: discarding one checks out its recorded commit (in it and its
    /// nested submodules), which also drops edits made inside it.
    fn confirm_discard(
        &mut self,
        tracked: Vec<std::path::PathBuf>,
        untracked: Vec<std::path::PathBuf>,
    ) -> Task<Message> {
        // Whether a copy fits is measured off the UI thread: an untracked
        // folder can hold thousands of files.
        let Some((repo, workdir)) = self
            .active_tab()
            .map(|t| (t.repo.id(), t.repo.cwd().to_path_buf()))
        else {
            return Task::none();
        };
        let workdir = Some(workdir);
        let files: Vec<_> = tracked.iter().chain(&untracked).cloned().collect();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut saved = gf_git::Scope {
                        files,
                        ..gf_git::Scope::default()
                    };
                    workdir.is_some_and(|w| saved.expand_files(&w).is_ok())
                })
                .await
                .unwrap_or(false)
            },
            move |fits| Message::DiscardMeasured {
                repo,
                tracked: tracked.clone(),
                untracked: untracked.clone(),
                fits,
            },
        )
    }

    /// The discard confirmation, once [`Self::confirm_discard`] knows
    /// whether a copy can be kept for undo (`fits`).
    pub(super) fn show_discard(
        &mut self,
        repo: gf_core::model::RepoId,
        tracked: Vec<std::path::PathBuf>,
        untracked: Vec<std::path::PathBuf>,
        fits: bool,
    ) -> Task<Message> {
        // The confirmation acts on the active tab: not on another one the
        // user switched to while the size was measured.
        if self.active_tab().map(|t| t.repo.id()) != Some(repo) {
            return Task::none();
        }
        let is_submodule = |p: &std::path::PathBuf| {
            self.active_tab()
                .and_then(|t| t.snapshot.as_ref())
                .is_some_and(|s| s.submodules.iter().any(|m| &m.path == p))
        };
        let (subs, files): (Vec<_>, Vec<_>) = tracked.iter().cloned().partition(is_submodule);
        let mut body = Vec::new();
        if !files.is_empty() {
            body.push(format!("Throw away unstaged changes in {}?", names(&files)));
        }
        if !subs.is_empty() {
            body.push(format!(
                "Put submodule {} back on the commit this repository records?",
                names(&subs)
            ));
            body.push(
                "Its checked-out commit (and those of submodules inside it) is switched back and edits to tracked files in it are thrown away; untracked files in it are kept."
                    .into(),
            );
        }
        if !untracked.is_empty() {
            body.push(format!("Delete untracked {}?", names(&untracked)));
        }
        body.push("Staged changes are kept.".into());
        // Discarded content is saved for undo unless there is too much.
        if !fits {
            body.push(
                "Too much to keep a copy of (over 64 MB or 5000 files): this can't be undone."
                    .into(),
            );
        } else if !subs.is_empty() {
            // Undo puts the submodule back on its commit, but no copy is
            // kept of the files inside it.
            body.push("Edits inside the submodule can't be brought back with undo.".into());
        }
        let confirm = Confirm::delete(
            "Discard changes",
            body,
            "Discard",
            Purpose::Discard {
                tracked,
                untracked,
                saved: fits,
            },
        );
        self.confirm(if fits && subs.is_empty() {
            confirm.undoable()
        } else {
            confirm
        })
    }

    pub(super) fn on_diff(
        &mut self,
        role: diff_view::Role,
        out: diff_view::Output,
    ) -> Task<Message> {
        match out {
            diff_view::Output::Apply {
                patches,
                action,
                description,
            } => {
                if action == Action::Discard {
                    self.confirm(
                        Confirm::delete(
                            "Discard changes",
                            vec![format!("{description} from the working tree?")],
                            "Discard",
                            Purpose::ApplyPatches {
                                patches,
                                action,
                                description,
                            },
                        )
                        .undoable(),
                    )
                } else {
                    self.apply_patches(patches, action, description)
                }
            }
            diff_view::Output::Highlight(file) => {
                let Some(tab) = self.active_tab() else {
                    return Task::none();
                };
                let id = tab.repo.id();
                tab.diff(role)
                    .highlight_file(file)
                    .map(move |done| Message::DiffFinished(id, role, done))
            }
            diff_view::Output::Copy { text, what } => self.copy_text(text, &what),
            diff_view::Output::Menu(menu) => self.open_menu(menu.map(|m| match role {
                diff_view::Role::Local => Message::LocalDiff(m),
                diff_view::Role::Commit => Message::CommitDiff(m),
            })),
            diff_view::Output::DiscardSubmodule(path) => {
                self.confirm_discard(vec![path], Vec::new())
            }
            diff_view::Output::Blame { path, rev } => self.open_blame(path, rev),
            diff_view::Output::FileHistory(path) => self.open_file_history(path),
            diff_view::Output::Reload => {
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                match tab.diff(role).source().cloned() {
                    Some(source) => load_diff(tab, source),
                    None => Task::none(),
                }
            }
        }
    }

    pub(super) fn apply_patches(
        &mut self,
        patches: Vec<String>,
        action: Action,
        description: String,
    ) -> Task<Message> {
        let (cached, reverse) = action.apply_mode();
        // Partial staging changes the index entry, a partial discard the file.
        let record = self
            .active_tab()
            .and_then(|t| t.diff(t.code_view).current_path())
            .map(|p| {
                if action == Action::Discard {
                    // The dialog promised ⌘Z.
                    Record::new(undo::files(vec![p])).required()
                } else {
                    Record::new(undo::index(vec![p]))
                }
            });
        self.op_recorded(description, OpKind::Generic, record, move |r| async move {
            for p in patches {
                r.apply_patch(p, cached, reverse).await?;
            }
            Ok(())
        })
    }

    pub(super) fn on_commit(&mut self, out: commit_box::Output) -> Task<Message> {
        match out {
            commit_box::Output::Commit {
                message,
                amend: true,
            } => {
                let head = self
                    .active_tab()
                    .and_then(|t| t.snapshot.as_ref())
                    .and_then(|s| s.status.head.oid.as_ref())
                    .map_or_else(|| "HEAD".to_owned(), |o| o.short().to_owned());
                let what = if message.trim().is_empty() {
                    "with the staged changes added (message kept)"
                } else {
                    "with the staged changes and this message"
                };
                let mut body = vec![format!("Replace commit {head} with a new one {what}?")];
                if let Some(on) = self.head_on_remote() {
                    body.push(format!(
                        "⚠ It is already on {on}: publishing the new one needs a force push."
                    ));
                }
                self.confirm(Confirm::change(
                    "amend",
                    "Amend last commit",
                    body,
                    "Amend",
                    Purpose::Amend { message },
                ))
            }
            commit_box::Output::Commit { message, .. } => {
                let summary = message.lines().next().unwrap_or_default().to_owned();
                let record = Record::head(gf_git::HeadMode::Soft).with_message(message.clone());
                self.op_recorded(
                    format!("commit: {summary}"),
                    OpKind::Commit,
                    Some(record),
                    move |r| async move {
                        r.commit(&message, &CommitOptions::default())
                            .await
                            .map(|_| ())
                    },
                )
            }
            commit_box::Output::LoadHeadMessage => {
                let Some(tab) = self.active_tab() else {
                    return Task::none();
                };
                let repo = tab.repo.clone();
                let id = repo.id();
                Task::perform(
                    async move { repo.head_message().await.ok().flatten() },
                    move |m| Message::AmendTextLoaded(id, m),
                )
            }
        }
    }

    pub(super) fn on_history(&mut self, out: history::Output) -> Task<Message> {
        match out {
            history::Output::Collapse => self.on_shortcut(Shortcut::ToggleHistory),
            history::Output::ShowCommit(oid) => self.show_diff(DiffSource::Commit(oid)),
            history::Output::ToggleAllBranches => {
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                tab.all_branches = !tab.all_branches;
                let id = tab.repo.id();
                self.request_parts(id, Parts::HISTORY)
            }
            history::Output::LoadMore => {
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                match tab.history.revision().map(str::to_owned) {
                    Some(revision) => load_history(tab, revision),
                    // Coalesces with a refresh already running.
                    None => {
                        let id = tab.repo.id();
                        self.request_parts(id, Parts::HISTORY)
                    }
                }
            }
            history::Output::BackToHead => {
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                tab.sidebar.clear_selection();
                tab.history.set_revision(None);
                Task::none()
            }
            history::Output::NewBranchAt(oid) => {
                self.new_branch_dialog(Some(oid.to_string()), true)
            }
            history::Output::NewTagAt(oid) => self.new_tag_dialog(Some(oid.to_string())),
            history::Output::CheckoutDetached(oid) => self.confirm(Confirm::change(
                "checkout",
                "Check out commit",
                vec![
                    format!(
                        "Switch to {} without a branch (detached HEAD)?",
                        oid.short()
                    ),
                    "Your files change to that commit. Good for looking around;".into(),
                    "to keep new commits, create a branch.".into(),
                ],
                "Check out",
                Purpose::CheckoutDetached(oid.to_string()),
            )),
            history::Output::Act(action, oid) => self.on_commit_action(action, oid),
            history::Output::ShowRange(commits) => self.show_range(commits),
            history::Output::Menu { show, menu } => {
                let show =
                    show.map_or_else(Task::none, |oid| self.show_diff(DiffSource::Commit(oid)));
                let open = self.open_menu(menu.map(Message::History));
                Task::batch([show, open])
            }
            history::Output::Batch(action, commits) => self.on_batch(action, commits),
            history::Output::CopyId(oid) => {
                self.flash(NoteKind::Info, format!("copied {}", oid.short()));
                iced::clipboard::write(oid.to_string())
            }
        }
    }

    pub(super) fn on_sidebar(&mut self, out: sidebar::Output) -> Task<Message> {
        match out {
            sidebar::Output::ShowRevision(rev) => {
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                // A branch/tag's history is only visible in HISTORY mode.
                tab.code_view = diff_view::Role::Commit;
                tab.history.set_revision(rev.clone());
                let task = rev.map_or_else(Task::none, |r| load_history(tab, r));
                self.expand_history();
                task
            }
            sidebar::Output::ShowStash(s) => self.show_diff(DiffSource::Stash {
                index: s.index,
                id: s.id,
            }),
            sidebar::Output::Menu { show, menu } => {
                // A right-clicked item gets its history/diff like a click.
                let shown = match show {
                    Some(sidebar::Selection::Ref(r)) => {
                        self.on_sidebar(sidebar::Output::ShowRevision(Some(r)))
                    }
                    Some(sidebar::Selection::Stash(i)) => {
                        let stash = self
                            .active_tab()
                            .and_then(|t| t.snapshot.as_ref())
                            .and_then(|s| s.stashes.iter().find(|s| s.index == i).cloned());
                        stash.map_or_else(Task::none, |s| {
                            self.on_sidebar(sidebar::Output::ShowStash(s))
                        })
                    }
                    _ => Task::none(),
                };
                let open = self.open_menu(menu.map(Message::Sidebar));
                Task::batch([shown, open])
            }
            sidebar::Output::NewBranch { from, branch_here } => {
                self.new_branch_dialog(from, branch_here)
            }
            sidebar::Output::NewTag => self.new_tag_dialog(None),
            sidebar::Output::NewStash => self.stash_dialog(),
            sidebar::Output::Checkout(b) => self.checkout(b),
            sidebar::Output::RenameBranch(name) => self.open_dialog(
                crate::features::dialog::Dialog::confirm(
                    "Rename branch",
                    vec![format!("New name for `{name}`:")],
                    "Rename",
                    Purpose::RenameBranch { old: name.clone() },
                )
                .field("name", name, "new-name"),
            ),
            sidebar::Output::DeleteBranch(b) => self.delete_branch_dialog(&b),
            sidebar::Output::PushBranch(b) => self.push_branch(&b),
            sidebar::Output::DeleteTag(name) => self.confirm(
                Confirm::delete(
                    "Delete tag",
                    self.delete_tag_body(&name),
                    "Delete",
                    Purpose::DeleteTag(name),
                )
                .undoable(),
            ),
            sidebar::Output::PushTag(name) => {
                if !self.has_remote() {
                    return self.no_remote("push tag");
                }
                let remote = self.default_remote();
                self.confirm(Confirm::change(
                    "push-tag",
                    "Push tag",
                    vec![format!(
                        "Publish tag `{name}` to `{remote}`? Others will see it."
                    )],
                    "Push",
                    Purpose::PushTag { remote, name },
                ))
            }
            sidebar::Output::StashApply(i) => self.confirm(Confirm::change(
                "stash-apply",
                "Apply stash",
                vec![
                    format!("Apply stash@{{{i}}} to your working tree?"),
                    "The stash stays in the list. Conflicting files are left for you to resolve."
                        .into(),
                ],
                "Apply",
                Purpose::StashApply(i),
            )),
            sidebar::Output::StashPop(i) => self.confirm(Confirm::change(
                "stash-pop",
                "Pop stash",
                vec![
                    format!(
                        "Apply stash@{{{i}}} to your working tree and remove it from the list?"
                    ),
                    "If applying conflicts, the stash is kept.".into(),
                ],
                "Pop",
                Purpose::StashPop(i),
            )),
            sidebar::Output::OpenRepo(path) => {
                let root = self.active_tab().map(|t| t.repo.cwd().to_path_buf());
                // Submodule paths are relative to the superproject.
                let full = match root {
                    Some(r) if path.is_relative() => r.join(path),
                    _ => path,
                };
                self.update(Message::Open(full))
            }
            sidebar::Output::NewWorktree => self.new_worktree_dialog(),
            sidebar::Output::RemoveWorktree(path) => self.confirm(
                Confirm::delete(
                    "Remove worktree",
                    vec![
                        format!(
                            "Remove the working tree at {}?",
                            crate::widgets::format::tilde(&path)
                        ),
                        "Its folder is deleted; the branch and its commits stay.".into(),
                        "git refuses if it has uncommitted changes — tick force to remove anyway."
                            .into(),
                    ],
                    "Remove",
                    Purpose::RemoveWorktree(path),
                )
                .check("force (discard its uncommitted changes)", false),
            ),
            sidebar::Output::UpdateSubmodules => self.confirm(Confirm::change(
                "submodule-update",
                "Update submodules",
                vec![
                    "Check out every submodule at the commit this repository records?".into(),
                    "Uninitialized ones are fetched; local changes inside them may be overwritten."
                        .into(),
                ],
                "Update",
                Purpose::UpdateSubmodules,
            )),
            sidebar::Output::TrackLfs => self.open_dialog(
                crate::features::dialog::Dialog::confirm(
                    "Track with LFS",
                    vec!["Store matching files in Git LFS (edits .gitattributes):".into()],
                    "Track",
                    Purpose::TrackLfs,
                )
                .field("pattern", "", "*.psd"),
            ),
            sidebar::Output::UntrackLfs(pattern) => self.confirm(Confirm::change(
                "lfs-untrack",
                "Stop tracking with LFS",
                vec![
                    format!("Stop storing `{pattern}` in LFS (edits .gitattributes)?"),
                    "Files already committed stay in LFS history.".into(),
                ],
                "Untrack",
                Purpose::UntrackLfs(pattern),
            )),
            sidebar::Output::LfsPull => self.confirm(Confirm::change(
                "lfs-pull",
                "LFS pull",
                vec!["Download LFS file contents for the current checkout?".into()],
                "Pull",
                Purpose::LfsPull,
            )),
            sidebar::Output::Merge(branch) => self.ask_merge(branch),
            sidebar::Output::RebaseOnto(onto) => self.ask_rebase_onto(onto),
            sidebar::Output::Collapse => self.on_shortcut(Shortcut::ToggleRefs),
            sidebar::Output::StashDrop(i) => self.confirm(
                Confirm::delete(
                    "Drop stash",
                    vec![format!("Delete stash@{{{i}}} and the changes in it?")],
                    "Drop",
                    Purpose::StashDrop(i),
                )
                .undoable(),
            ),
        }
    }

    pub(super) fn on_toolbar(&mut self, m: toolbar::Message) -> Task<Message> {
        match m {
            toolbar::Message::Fetch => {
                self.sync_op("fetch all remotes", OpKind::Fetch, |r, opts| async move {
                    r.fetch(None, opts).await?;
                    // Which tags each remote has: what push compares with.
                    // Only a count depends on it, so a failure is logged.
                    // The fetch itself succeeded: nothing here fails it.
                    for remote in r.remotes().await.unwrap_or_default() {
                        if let Err(e) = r.record_remote_tags(&remote.name).await {
                            tracing::warn!(remote = %remote.name, error = %e, "could not list remote tags");
                        }
                    }
                    Ok(())
                })
            }
            toolbar::Message::Pull => {
                let has_upstream = self
                    .active_tab()
                    .and_then(|t| t.snapshot.as_ref())
                    .is_some_and(|s| s.status.head.upstream.is_some());
                if !has_upstream {
                    self.flash(
                        NoteKind::Error,
                        "this branch has no upstream — push it first to publish it",
                    );
                    return Task::none();
                }
                let (up, branch, behind) = self
                    .active_tab()
                    .and_then(|t| t.snapshot.as_ref())
                    .map(|s| {
                        let h = &s.status.head;
                        (
                            h.upstream.clone().unwrap_or_default(),
                            h.branch.clone().unwrap_or_default(),
                            h.behind,
                        )
                    })
                    .unwrap_or_default();
                self.confirm(Confirm::change(
                    "pull",
                    "Pull",
                    vec![
                        format!("Bring {behind} commit(s) from `{up}` into `{branch}`?"),
                        "Your files are updated; uses your pull.rebase / pull.ff setting.".into(),
                    ],
                    "Pull",
                    Purpose::Pull,
                ))
            }
            toolbar::Message::Push => {
                let head = self
                    .active_tab()
                    .and_then(|t| t.snapshot.as_ref())
                    .and_then(|s| s.head_branch().cloned());
                match head {
                    Some(b) => self.push_branch(&b),
                    None => {
                        self.flash(
                            NoteKind::Error,
                            "HEAD is detached — create a branch to push",
                        );
                        Task::none()
                    }
                }
            }
            toolbar::Message::Stash => self.stash_dialog(),
            toolbar::Message::Refresh => self.refresh_active(),
            toolbar::Message::Cancel => self.on_shortcut(Shortcut::Escape),
        }
    }

    pub(super) fn pull(&mut self, mode: PullMode) -> Task<Message> {
        self.sync_op("pull", OpKind::Pull, move |r, opts| async move {
            r.pull(mode, opts).await
        })
    }

    pub(super) fn push_branch(&mut self, b: &Branch) -> Task<Message> {
        if b.name.kind() != RefKind::LocalBranch {
            return Task::none();
        }
        let branch = b.name.short().to_owned();
        if b.upstream.is_none() {
            if !self.has_remote() {
                return self.no_remote("publish");
            }
            let remote = self.default_remote();
            return self.open_dialog(
                crate::features::dialog::Dialog::confirm(
                    "Publish branch",
                    vec![
                        format!("`{branch}` is not on any remote yet."),
                        "Push it and track it as upstream:".into(),
                    ],
                    "Publish",
                    Purpose::Publish {
                        branch,
                        tags: self.pending_tags(),
                    },
                )
                .field("remote", remote, "origin"),
            );
        }
        let (up, ahead) = b
            .upstream
            .as_ref()
            .map(|u| (u.name.short().to_owned(), u.ahead))
            .unwrap_or_default();
        // `git push` alone pushes the checked-out branch; any other branch
        // needs its remote and name spelled out.
        let to = (!b.is_head).then(|| self.push_target(b)).flatten();
        // Tags go with the checked-out branch, as the toolbar counts them.
        let tags = b.is_head.then(|| self.pending_tags()).flatten();
        let mut body = Vec::new();
        if ahead > 0 || tags.is_none() {
            body.push(format!("Send {ahead} commit(s) from `{branch}` to `{up}`."));
        }
        if let Some(t) = &tags {
            body.push(format!(
                "Send tag(s) {} to `{}`.",
                t.names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                t.remote
            ));
        }
        body.push("Others will see them.".into());
        self.confirm(Confirm::change(
            "push",
            "Push",
            body,
            "Push",
            Purpose::Push { branch, to, tags },
        ))
    }

    /// Remote and refspec of `b`'s upstream, when it is on a remote.
    fn push_target(&self, b: &Branch) -> Option<super::PushTo> {
        let up = b.upstream.as_ref()?;
        if up.name.kind() != RefKind::RemoteBranch {
            return None;
        }
        let short = up.name.short();
        let remotes = &self.active_tab()?.snapshot.as_ref()?.remotes;
        // Remote names may contain `/`: match the configured ones first.
        let (remote, branch) = remotes
            .iter()
            .filter_map(|r| {
                short
                    .strip_prefix(r.name.as_str())
                    .and_then(|rest| rest.strip_prefix('/'))
                    .map(|branch| (r.name.as_str(), branch))
            })
            .max_by_key(|(remote, _)| remote.len())
            .or_else(|| short.split_once('/'))?;
        Some(super::PushTo {
            remote: remote.to_owned(),
            refspec: format!("{}:{branch}", b.name.short()),
        })
    }

    fn checkout(&mut self, b: Branch) -> Task<Message> {
        let remote = b.name.kind() == RefKind::RemoteBranch;
        let name = b.name.short().to_owned();
        // Changes, a branch to leave them on and nothing in progress: ask
        // whether to leave them there (stash) or carry them over, after
        // checking which files would keep them from coming along.
        let leave = self
            .active_tab()
            .and_then(|t| t.snapshot.as_deref())
            .filter(|s| s.state.is_clean())
            .and_then(|s| {
                let from = s.status.head.branch.clone()?;
                let changed = undo::changed_paths(&s.status);
                (!changed.is_empty()).then_some((from, changed))
            });
        if let Some((from, changed)) = leave {
            return self.measure_switch(name, remote, from, changed);
        }
        let body = if remote {
            vec![
                format!("Create a local branch tracking `{name}` and switch to it?"),
                "Your files change to that branch's content.".into(),
            ]
        } else {
            vec![
                format!("Switch to `{name}`?"),
                "Your files change to that branch. Uncommitted changes come along".into(),
                "unless they conflict (then git refuses and nothing changes).".into(),
            ]
        };
        // Nothing to bring along or to leave behind: just switch (⌘Z goes
        // back). A remote branch also creates a local one, so it asks.
        let quiet = !remote
            && self
                .active_tab()
                .and_then(|t| t.snapshot.as_deref())
                .is_some_and(Snapshot::can_switch_quietly);
        self.confirm(
            Confirm::change(
                "checkout",
                "Switch branch",
                body,
                "Switch",
                Purpose::Checkout {
                    name,
                    remote,
                    leave_from: None,
                    blocked: Vec::new(),
                },
            )
            .undoable()
            .skip_if(quiet),
        )
    }

    fn measure_switch(
        &mut self,
        name: String,
        remote: bool,
        from: String,
        changed: Vec<std::path::PathBuf>,
    ) -> Task<Message> {
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        let (repo, r) = (tab.repo.id(), tab.repo.clone());
        let target = name.clone();
        let count = changed.len();
        Task::perform(
            async move {
                r.switch_blockers(&target, &changed)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |blocked| Message::SwitchMeasured {
                repo,
                name: name.clone(),
                remote,
                from: from.clone(),
                changed: count,
                blocked,
            },
        )
    }

    /// Asks what to do with the changes when switching branch: leave them
    /// here (stash) or carry them over, each with the git commands it runs.
    pub(super) fn show_switch(
        &mut self,
        repo: gf_core::model::RepoId,
        name: String,
        remote: bool,
        from: String,
        changed: usize,
        blocked: Result<Vec<std::path::PathBuf>, String>,
    ) -> Task<Message> {
        use crate::features::dialog::{Dialog, Opt, Severity, Tone};
        if self.active_tab().map(|t| t.repo.id()) != Some(repo) {
            return Task::none();
        }
        let switch = if remote {
            let local = name.split_once('/').map_or(name.as_str(), |(_, b)| b);
            format!("$ git switch --track -c {local} {name}")
        } else {
            format!("$ git switch {name}")
        };
        let earlier = self
            .active_tab()
            .and_then(|t| t.snapshot.as_deref())
            .map_or(0, |s| {
                s.stashes
                    .iter()
                    .filter(|e| gf_git::left_on_branch(&e.message) == Some(from.as_str()))
                    .count()
            });

        let mut leave = Opt::new("stash, then switch")
            .key('s')
            .line(
                format!("$ git stash push -u -m \"{}\"", gf_git::left_on(&from)),
                Tone::Command,
            )
            .line(&switch, Tone::Command)
            .line(
                format!("offered back when you return to {from}"),
                Tone::Hint,
            );
        if earlier > 0 {
            leave = leave.line(
                format!("{earlier} earlier stash(es) left on {from} stay in the list"),
                Tone::Hint,
            );
        }
        let mut carry = Opt::new("carry changes over")
            .key('c')
            .line(&switch, Tone::Command);
        let blocked = match blocked {
            Ok(b) if b.is_empty() => {
                carry = carry.line("✓ nothing in the way", Tone::Hint);
                b
            }
            Ok(b) => {
                carry = carry
                    .line(
                        format!("✗ {} file(s) would block it — git would refuse", b.len()),
                        Tone::Problem,
                    )
                    .line(blocker_summary(&b), Tone::Problem)
                    .disabled(true);
                b
            }
            Err(e) => {
                carry = carry.line(
                    format!("could not check what is in the way: {e}"),
                    Tone::Hint,
                );
                Vec::new()
            }
        };

        let to = if remote {
            format!("{name} (as a new tracking branch)")
        } else {
            name.clone()
        };
        let body = vec![
            format!("{changed} changed file(s) on `{from}`"),
            "⌘Z undoes it afterwards (stash and switch together)".into(),
        ];
        self.open_dialog(
            Dialog::confirm(
                format!("switch {from} → {to}"),
                body,
                "Switch",
                Purpose::Checkout {
                    name,
                    remote,
                    leave_from: Some(from),
                    blocked,
                },
            )
            .severity(Severity::Change)
            .options("your changes:", vec![leave, carry], 0)
            .undoable(),
        )
    }

    /// `l` in the switch dialog: every blocking file, in the command log.
    pub(super) fn list_switch_blockers(&mut self) -> bool {
        let Some(crate::features::dialog::Dialog {
            purpose: Purpose::Checkout { name, blocked, .. },
            ..
        }) = self.dialog.as_ref()
        else {
            return false;
        };
        if blocked.is_empty() {
            return false;
        }
        let title = format!(
            "{} file(s) keep changes from coming to {name}",
            blocked.len()
        );
        let lines = blocked.iter().map(|p| p.display().to_string()).collect();
        self.log.note(NoteKind::Info, title, lines);
        if !self.settings.appearance.show_command_log {
            self.flash(NoteKind::Info, "listed in the command log (⌘J to show it)");
        }
        true
    }

    /// First configured remote, preferring `origin`.
    /// Local tags the push remote does not have; `None` when there are
    /// none or gf does not know that remote's tags.
    fn pending_tags(&self) -> Option<crate::repo_tab::PendingTags> {
        self.active_tab()?
            .snapshot
            .as_ref()?
            .pending_tags
            .clone()
            .filter(|t| !t.names.is_empty())
    }

    /// ⌘C: the selected lines of the diff on screen (nothing selected:
    /// nothing copied, so a text field's own ⌘C is never overwritten).
    pub(super) fn copy_diff_selection(&mut self) -> Task<Message> {
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        let diff = match tab.code_view {
            diff_view::Role::Local => &tab.local_diff,
            diff_view::Role::Commit => &tab.commit_diff,
        };
        match diff.selected_text() {
            Some((text, what)) => self.copy_text(text, &what),
            None => Task::none(),
        }
    }

    fn copy_text(&mut self, text: String, what: &str) -> Task<Message> {
        self.flash(NoteKind::Info, format!("copied {what}"));
        iced::clipboard::write(text)
    }

    /// Whether the active repository has any remote configured.
    pub(super) fn has_remote(&self) -> bool {
        self.active_tab()
            .and_then(|t| t.snapshot.as_ref())
            .is_some_and(|s| !s.remotes.is_empty())
    }

    /// Where HEAD's commit is known to be on a remote (`` `origin/main` ``
    /// or "a remote"), `None` when it is not, or gf cannot tell. Dialogs say
    /// a commit was pushed only when this knows it.
    pub(super) fn head_on_remote(&self) -> Option<String> {
        let snap = self.active_tab()?.snapshot.as_ref()?;
        head_on_remote(snap)
    }

    /// Delete-tag text: a local delete never touches a remote, so it is
    /// mentioned only when there is one (gf does not know which tags it has).
    fn delete_tag_body(&self, name: &str) -> Vec<String> {
        let mut body = vec![format!("Delete local tag `{name}`?")];
        if self.has_remote() {
            body.push(format!(
                "Remotes are not changed: if you pushed it to `{}`, it stays there.",
                self.default_remote()
            ));
        }
        body
    }

    /// `what` needs a remote and there is none: say so instead of offering
    /// a remote that does not exist.
    fn no_remote(&mut self, what: &str) -> Task<Message> {
        self.flash(
            NoteKind::Info,
            format!("{what}: no remote yet — add one with `remote add origin <url>`"),
        );
        Task::none()
    }

    pub(super) fn default_remote(&self) -> String {
        let remotes = self
            .active_tab()
            .and_then(|t| t.snapshot.as_ref())
            .map(|s| &s.remotes);
        remotes
            .and_then(|r| r.iter().find(|r| r.name == "origin").or_else(|| r.first()))
            .map_or_else(|| "origin".to_owned(), |r| r.name.clone())
    }

    pub(super) fn run_prompt_command(&mut self, cmd: PromptCommand) -> Task<Message> {
        match cmd {
            // On the welcome screen the recent list is numbered: `2⏎` opens #2.
            PromptCommand::Git(args) if self.tabs.is_empty() && is_number(&args) => {
                let n: usize = args[0].parse().unwrap_or(0);
                match self.settings.repos.recent.get(n.wrapping_sub(1)).cloned() {
                    Some(path) => self.update(Message::Open(path)),
                    None => {
                        self.flash(NoteKind::Error, format!("no recent repository #{n}"));
                        Task::none()
                    }
                }
            }
            PromptCommand::Git(args) => self.run_git(args),
            PromptCommand::Open(None) => self.open_picker(),
            PromptCommand::Open(Some(path)) => self.update(Message::Open(path)),
            PromptCommand::CloseTab => self.close_tab(self.active),
            PromptCommand::SelectTab(i) => {
                if i >= self.tabs.len() {
                    self.flash(
                        NoteKind::Error,
                        format!("no tab {} — {} open", i + 1, self.tabs.len()),
                    );
                }
                self.select_tab(i)
            }
            PromptCommand::Refresh => self.refresh_active(),
            PromptCommand::Settings => self.toggle_settings(),
            PromptCommand::Theme(name) => {
                match name {
                    Some(n) if theme::PRESET_NAMES.contains(&n.as_str()) => self.set_theme(&n),
                    Some(n) => {
                        self.flash(
                            NoteKind::Error,
                            format!(
                                "unknown theme {n} — try: {}",
                                theme::PRESET_NAMES.join(", ")
                            ),
                        );
                    }
                    None => self.cycle_theme(false),
                }
                Task::none()
            }
            PromptCommand::ToggleLog => self.on_shortcut(Shortcut::ToggleLog),
            PromptCommand::ToggleRefs => self.on_shortcut(Shortcut::ToggleRefs),
            PromptCommand::ToggleHistory => self.on_shortcut(Shortcut::ToggleHistory),
            PromptCommand::ToggleLocal => self.on_shortcut(Shortcut::ToggleLocal),
            PromptCommand::ClearLog => {
                self.log.clear();
                Task::none()
            }
            PromptCommand::Help => {
                let mut lines: Vec<String> =
                    vec!["Type any git command (the `git` prefix is optional): status, log -5, switch dev …".into()];
                lines.extend(
                    BUILTINS
                        .iter()
                        .map(|(name, help)| format!("{name:<10} {help}")),
                );
                lines.push(
                    "prompt: ↑/↓ history · ⇥ complete · ⌘⏎ commit · Esc leave the prompt".into(),
                );
                lines.push(
                    "panels: ↑/↓ PgUp/PgDn Home/End move · ←/→ ⇥ switch panel · / prompt".into(),
                );
                lines.push(
                    "  history ⇧↑/↓ range · . actions · working tree Space stage · d discard · diff n/p hunk"
                        .into(),
                );
                lines.push("working tree . then v opens the full file · code view ⌘A select all · ⌘C copy · Esc close".into());
                lines.push(
                    "click a ref for its history · click diff lines to stage just those".into(),
                );
                self.log.note(NoteKind::Info, "help", lines);
                self.settings.appearance.show_command_log = true;
                operation::focus(PROMPT_ID)
            }
            PromptCommand::Confirms { reset: true } => {
                let n = self.settings.confirm.skip.len();
                self.settings.confirm.skip.clear();
                self.save_settings();
                self.flash(
                    NoteKind::Info,
                    format!("all confirmations restored ({n} were skipped)"),
                );
                Task::none()
            }
            PromptCommand::Confirms { reset: false } => {
                let lines = if self.settings.confirm.skip.is_empty() {
                    vec!["every change asks for confirmation (nothing skipped)".into()]
                } else {
                    let mut l = vec![format!(
                        "not asking before: {}",
                        self.settings.confirm.skip.join(", ")
                    )];
                    l.push("`:confirm reset` to be asked again; deletions always ask".into());
                    l
                };
                self.log.note(NoteKind::Info, "confirmations", lines);
                self.settings.appearance.show_command_log = true;
                Task::none()
            }
            PromptCommand::GixFetch(on) => {
                if let Some(on) = on {
                    self.settings.git.gix_fetch = on;
                    self.save_settings();
                    for tab in &mut self.tabs {
                        tab.repo = tab.repo.clone().with_gix_fetch(on);
                    }
                }
                let text = if self.settings.git.gix_fetch {
                    "fetch uses gix (git when gix can't) — `:gix-fetch off` to use git"
                } else {
                    "fetch uses git — `:gix-fetch on` to fetch in-process with gix"
                };
                self.flash(NoteKind::Info, text);
                Task::none()
            }
            PromptCommand::Undo { list: false } => self.undo_steps(false, 1),
            PromptCommand::Undo { list: true } => self.toggle_undo_list(),
            PromptCommand::Redo => self.undo_steps(true, 1),
            PromptCommand::Pwd => {
                let text = self.active_tab().map_or_else(
                    || "no repository open".to_owned(),
                    |t| t.repo.cwd().display().to_string(),
                );
                self.log.note(NoteKind::Info, "pwd", vec![text]);
                self.settings.appearance.show_command_log = true;
                Task::none()
            }
            PromptCommand::Quit => iced::exit(),
        }
    }
}

fn is_number(args: &[String]) -> bool {
    matches!(args, [one] if !one.is_empty() && one.bytes().all(|b| b.is_ascii_digit()))
}

/// `a.txt` or `a.txt and 2 more`.
pub(super) fn names(paths: &[std::path::PathBuf]) -> String {
    match paths {
        [] => "nothing".into(),
        [one] => one.display().to_string(),
        [first, rest @ ..] => format!("{} and {} more", first.display(), rest.len()),
    }
}

/// Lets `StashOptions` be built by the dialog module.
pub(super) fn stash_options(
    message: String,
    include_untracked: bool,
    keep_index: bool,
) -> StashOptions {
    StashOptions {
        message: (!message.is_empty()).then_some(message),
        include_untracked,
        keep_index,
        paths: Vec::new(),
    }
}

/// Shows `path` selected in the file manager (Finder on macOS).
fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    // Tests check the path exists instead of opening Finder windows.
    if cfg!(test) {
        return std::fs::metadata(path).map(|_| ());
    }
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    let mut child = cmd.spawn()?;
    // Reaped in the background so it never lingers as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// `a.rs, b.rs, c.rs  +11 more  [l] list`: the first few blocking files.
fn blocker_summary(blocked: &[std::path::PathBuf]) -> String {
    const SHOWN: usize = 3;
    let names: Vec<String> = blocked
        .iter()
        .take(SHOWN)
        .map(|p| p.display().to_string())
        .collect();
    let mut s = names.join(", ");
    if blocked.len() > SHOWN {
        s.push_str(&format!("  +{} more  [l] list", blocked.len() - SHOWN));
    }
    s
}

/// Where HEAD's commit is known to be on a remote: on its upstream when
/// the branch is not ahead of it, or on some remote when nothing is left to
/// publish. `None` without remotes, before the first commit, or when HEAD
/// has commits no remote has.
fn head_on_remote(snap: &crate::repo_tab::Snapshot) -> Option<String> {
    let head = &snap.status.head;
    if snap.remotes.is_empty() || head.oid.is_none() {
        return None;
    }
    match (&head.upstream, snap.unpublished) {
        (Some(up), _) if head.ahead == 0 => Some(format!("`{up}`")),
        (None, Some(0)) => Some("a remote".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_blockers_show_three_and_a_count() {
        let p = |s: &str| std::path::PathBuf::from(s);
        assert_eq!(blocker_summary(&[p("a.rs")]), "a.rs");
        let many: Vec<_> = ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"].map(p).into();
        assert_eq!(
            blocker_summary(&many),
            "a.rs, b.rs, c.rs  +2 more  [l] list"
        );
    }

    #[test]
    fn a_commit_counts_as_pushed_only_when_known() {
        use crate::repo_tab::Snapshot;
        use gf_core::model::Remote;
        let mut snap = Snapshot::default();
        snap.status.head.oid = Some("a".repeat(40).parse().expect("oid"));
        snap.unpublished = Some(0);
        assert_eq!(head_on_remote(&snap), None, "no remote: never pushed");

        snap.remotes = vec![Remote {
            name: "origin".into(),
            fetch_url: "https://example.com/r.git".into(),
            push_url: None,
        }];
        assert_eq!(head_on_remote(&snap).as_deref(), Some("a remote"));
        snap.unpublished = Some(2);
        assert_eq!(head_on_remote(&snap), None, "HEAD is not on any remote");

        snap.unpublished = None;
        snap.status.head.upstream = Some("origin/main".into());
        assert_eq!(head_on_remote(&snap).as_deref(), Some("`origin/main`"));
        snap.status.head.ahead = 1;
        assert_eq!(head_on_remote(&snap), None, "HEAD is ahead of it");
    }
}
