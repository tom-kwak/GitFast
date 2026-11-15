//! Reactions to feature outputs: each feature asks, the app decides what
//! git work to run (directly, or after a confirmation dialog).

use gf_core::model::{Branch, RefKind};
use gf_git::{CommitOptions, PullMode, PushOptions, StashOptions};
use iced::Task;
use iced::widget::operation;

use super::dialogs::Purpose;
use super::ops::{load_diff, load_history, load_snapshot};
use super::{App, Message, OpKind, Shortcut};
use crate::features::command_log::NoteKind;
use crate::features::diff_view::{self, Action, DiffSource};
use crate::features::prompt::PROMPT_ID;
use crate::features::prompt::parse::{BUILTINS, PromptCommand};
use crate::features::{changes, commit_box, history, sidebar, toolbar};
use crate::theme;

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
        tab.diff_mut(role).show(source);
        load_diff(tab, role)
    }

    pub(super) fn on_changes(&mut self, out: changes::Output) -> Task<Message> {
        match out {
            changes::Output::ShowDiff(source) => self.show_diff(source),
            changes::Output::Stage(paths) => {
                let label = format!("stage {}", names(&paths));
                self.op(label, OpKind::Generic, move |r| async move {
                    r.stage(&paths).await
                })
            }
            changes::Output::Unstage(paths) => {
                let label = format!("unstage {}", names(&paths));
                self.op(label, OpKind::Generic, move |r| async move {
                    r.unstage(&paths).await
                })
            }
            changes::Output::StageAll => self.op("stage all", OpKind::Generic, |r| async move {
                r.stage_all().await
            }),
            changes::Output::UnstageAll => {
                self.op("unstage all", OpKind::Generic, |r| async move {
                    r.unstage_all().await
                })
            }
            changes::Output::Discard { tracked, untracked } => {
                let mut body = Vec::new();
                if !tracked.is_empty() {
                    body.push(format!(
                        "Throw away unstaged changes in {}?",
                        names(&tracked)
                    ));
                }
                if !untracked.is_empty() {
                    body.push(format!("Delete untracked {}?", names(&untracked)));
                }
                body.push("Staged changes are kept.".into());
                self.open_dialog(
                    crate::features::dialog::Dialog::confirm(
                        "Discard changes",
                        body,
                        "Discard",
                        Purpose::Discard { tracked, untracked },
                    )
                    .danger(),
                )
            }
        }
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
                    self.open_dialog(
                        crate::features::dialog::Dialog::confirm(
                            "Discard changes",
                            vec![format!("{description} from the working tree?")],
                            "Discard",
                            Purpose::ApplyPatches {
                                patches,
                                action,
                                description,
                            },
                        )
                        .danger(),
                    )
                } else {
                    self.apply_patches(patches, action, description)
                }
            }
            diff_view::Output::Highlight(file, diff) => {
                let Some(tab) = self.active_tab() else {
                    return Task::none();
                };
                let Some(source) = tab.diff(role).source().cloned() else {
                    return Task::none();
                };
                let id = tab.repo.id();
                Task::perform(diff_view::highlight(*diff), move |spans| {
                    Message::DiffHighlighted(id, source.clone(), file, spans)
                })
            }
            diff_view::Output::Reload => match self.active_tab() {
                Some(tab) => load_diff(tab, role),
                None => Task::none(),
            },
        }
    }

    pub(super) fn apply_patches(
        &mut self,
        patches: Vec<String>,
        action: Action,
        description: String,
    ) -> Task<Message> {
        let (cached, reverse) = match action {
            Action::Stage => (true, false),
            Action::Unstage => (true, true),
            Action::Discard => (false, true),
        };
        self.op(description, OpKind::Generic, move |r| async move {
            for p in patches {
                r.apply_patch(p, cached, reverse).await?;
            }
            Ok(())
        })
    }

    pub(super) fn on_commit(&mut self, out: commit_box::Output) -> Task<Message> {
        match out {
            commit_box::Output::Commit { message, amend } => {
                let label = if amend { "amend commit" } else { "commit" };
                let summary = message.lines().next().unwrap_or_default().to_owned();
                self.op(
                    format!("{label}: {summary}"),
                    OpKind::Commit,
                    move |r| async move {
                        let opts = CommitOptions {
                            amend,
                            ..CommitOptions::default()
                        };
                        r.commit(&message, &opts).await.map(|_| ())
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
            history::Output::ShowCommit(oid) => self.show_diff(DiffSource::Commit(oid)),
            history::Output::ToggleAllBranches => {
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                tab.all_branches = !tab.all_branches;
                load_snapshot(tab)
            }
            history::Output::BackToHead => {
                let Some(tab) = self.active_tab_mut() else {
                    return Task::none();
                };
                tab.sidebar.clear_selection();
                tab.history.set_revision(None);
                Task::none()
            }
            history::Output::NewBranchAt(oid) => self.new_branch_dialog(Some(oid.to_string())),
            history::Output::NewTagAt(oid) => self.new_tag_dialog(Some(oid.to_string())),
            history::Output::CheckoutDetached(oid) => {
                self.open_dialog(crate::features::dialog::Dialog::confirm(
                    "Check out commit",
                    vec![
                        format!(
                            "Switch to {} without a branch (detached HEAD)?",
                            oid.short()
                        ),
                        "Good for looking around. To keep new commits, create a branch.".into(),
                    ],
                    "Check out",
                    Purpose::CheckoutDetached(oid.to_string()),
                ))
            }
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
                tab.history.set_revision(rev.clone());
                rev.map_or_else(Task::none, |r| load_history(tab, r))
            }
            sidebar::Output::ShowStash(s) => self.show_diff(DiffSource::Stash {
                index: s.index,
                id: s.id,
            }),
            sidebar::Output::NewBranch { from } => self.new_branch_dialog(from),
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
            sidebar::Output::DeleteTag(name) => self.open_dialog(
                crate::features::dialog::Dialog::confirm(
                    "Delete tag",
                    vec![
                        format!("Delete local tag `{name}`?"),
                        "Tags already pushed stay on the remote.".into(),
                    ],
                    "Delete",
                    Purpose::DeleteTag(name),
                )
                .danger(),
            ),
            sidebar::Output::PushTag(name) => {
                let remote = self.default_remote();
                self.op(
                    format!("push tag {name}"),
                    OpKind::Generic,
                    move |r| async move { r.push_tag(&remote, &name).await },
                )
            }
            sidebar::Output::StashApply(i) => self.op(
                format!("apply stash@{{{i}}}"),
                OpKind::Generic,
                move |r| async move { r.stash_apply(i).await },
            ),
            sidebar::Output::StashPop(i) => self.op(
                format!("pop stash@{{{i}}}"),
                OpKind::Generic,
                move |r| async move { r.stash_pop(i).await },
            ),
            sidebar::Output::StashDrop(i) => self.open_dialog(
                crate::features::dialog::Dialog::confirm(
                    "Drop stash",
                    vec![format!("Delete stash@{{{i}}}? Its changes will be lost.")],
                    "Drop",
                    Purpose::StashDrop(i),
                )
                .danger(),
            ),
        }
    }

    pub(super) fn on_toolbar(&mut self, m: toolbar::Message) -> Task<Message> {
        match m {
            toolbar::Message::Fetch => {
                self.sync_op("fetch all remotes", OpKind::Fetch, |r, opts| async move {
                    r.fetch(None, opts).await
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
                self.pull(PullMode::Config)
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
            toolbar::Message::NewBranch => self.new_branch_dialog(None),
            toolbar::Message::Stash => self.stash_dialog(),
            toolbar::Message::Refresh => self.refresh_active(),
            toolbar::Message::Cancel => self.on_shortcut(Shortcut::Escape),
            toolbar::Message::ToggleRefs => self.on_shortcut(Shortcut::ToggleRefs),
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
            let remote = self.default_remote();
            return self.open_dialog(
                crate::features::dialog::Dialog::confirm(
                    "Publish branch",
                    vec![
                        format!("`{branch}` is not on any remote yet."),
                        "Push it and track it as upstream:".into(),
                    ],
                    "Publish",
                    Purpose::Publish { branch },
                )
                .field("remote", remote, "origin"),
            );
        }
        let opts = PushOptions::default();
        let label = format!("push {branch}");
        let kind = OpKind::Push {
            branch: Some(branch),
        };
        self.sync_op(label, kind, move |r, run| async move {
            r.push(&opts, run).await
        })
    }

    fn checkout(&mut self, b: Branch) -> Task<Message> {
        let remote = b.name.kind() == RefKind::RemoteBranch;
        let name = b.name.short().to_owned();
        let label = if remote {
            format!("check out {name} as a tracking branch")
        } else {
            format!("switch to {name}")
        };
        self.op(label, OpKind::Generic, move |r| async move {
            r.switch_branch(&name, remote).await
        })
    }

    /// First configured remote, preferring `origin`.
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
            PromptCommand::Open(None) => self.update(Message::PickFolder),
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
            PromptCommand::Theme(name) => {
                let next = match name {
                    Some(n) if theme::PRESET_NAMES.contains(&n.as_str()) => n,
                    Some(n) => {
                        self.flash(
                            NoteKind::Error,
                            format!(
                                "unknown theme {n} — try: {}",
                                theme::PRESET_NAMES.join(", ")
                            ),
                        );
                        return Task::none();
                    }
                    None => {
                        let i = theme::PRESET_NAMES
                            .iter()
                            .position(|p| *p == self.tokens.name)
                            .map_or(0, |i| (i + 1) % theme::PRESET_NAMES.len());
                        theme::PRESET_NAMES[i].to_owned()
                    }
                };
                self.tokens =
                    theme::preset(&next).with_font_size(self.settings.appearance.font_size);
                self.flash(NoteKind::Info, format!("theme: {next}"));
                self.settings.appearance.theme = next;
                self.save_settings();
                Task::none()
            }
            PromptCommand::ToggleLog => self.on_shortcut(Shortcut::ToggleLog),
            PromptCommand::ToggleRefs => self.on_shortcut(Shortcut::ToggleRefs),
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
                lines.push("↑/↓ history · ⇥ complete · ⌘⏎ commit · Esc cancel/close".into());
                lines.push(
                    "click a ref for its history · click diff lines to stage just those".into(),
                );
                self.log.note(NoteKind::Info, "help", lines);
                self.settings.appearance.show_command_log = true;
                operation::focus(PROMPT_ID)
            }
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
fn names(paths: &[std::path::PathBuf]) -> String {
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
    }
}
