//! Undo / redo in the app (the history itself is `crate::undo`).
//!
//! ⌘Z (or the status line, or the history list) checks the repository
//! against the step first (`UndoChecked`):
//!
//! - it still looks like right after the step, and the step does not move
//!   HEAD (staging, discarding, branches, tags, stash): undone at once —
//!   redo brings it back, so nothing is lost;
//! - it moves HEAD (commit, merge, reset, checkout…): asked with the shared
//!   change confirmation (can be turned off), offering to stash
//!   uncommitted work first;
//! - something changed since (in gf, an editor or the terminal): asked
//!   with a delete confirmation that lists what would be overwritten.

use gf_core::model::RepoId;
use gf_git::{Capture, HeadMode, StashOptions};
use iced::Task;

use super::dialogs::{Confirm, Purpose};
use super::{App, Message, OpKind};
use crate::features::command_log::NoteKind;
use crate::features::undo_list;
use crate::undo::{Queue, Step};
use crate::widgets::format::truncate;

fn verb(redo: bool) -> &'static str {
    if redo { "redo" } else { "undo" }
}

impl App {
    /// A finished operation's step: on the undo stack, saved.
    pub(super) fn record_step(&mut self, id: RepoId, step: Step) {
        if let Some(tab) = self.tab_mut(id) {
            tab.undo.push(step);
            tab.undo.save(&tab.repo.handle().git_dir);
        }
    }

    /// Undoes (or redoes) the next `n` steps, one after the other.
    pub(super) fn undo_steps(&mut self, redo: bool, n: usize) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        if tab.undo_busy {
            self.flash(NoteKind::Info, format!("{} still running", verb(redo)));
            return Task::none();
        }
        tab.undo_queue = (n > 1).then_some(Queue { redo, left: n - 1 });
        self.check_next(redo)
    }

    /// Reads the repository for the next step's guard.
    fn check_next(&mut self, redo: bool) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        let Some(step) = tab.undo.next(redo).cloned() else {
            tab.undo_queue = None;
            let what = if redo {
                "nothing to redo"
            } else {
                "nothing to undo"
            };
            self.flash(NoteKind::Info, what);
            return Task::none();
        };
        let in_progress = tab.snapshot.as_ref().is_some_and(|s| !s.state.is_clean());
        if step.moves_head() && in_progress {
            tab.undo_queue = None;
            self.flash(
                NoteKind::Error,
                format!(
                    "can't {} \"{}\" in the middle of a merge or rebase — continue or abort it first",
                    verb(redo),
                    truncate(&step.label, 40)
                ),
            );
            return Task::none();
        }
        tab.undo_busy = true;
        let repo = tab.repo.clone();
        let id = repo.id();
        Task::perform(
            async move {
                repo.capture(&step.scope)
                    .await
                    .map(Box::new)
                    .map_err(|e| e.to_string())
            },
            move |now| Message::UndoChecked {
                repo: id,
                redo,
                now,
            },
        )
    }

    pub(super) fn on_undo_checked(
        &mut self,
        id: RepoId,
        redo: bool,
        now: Result<Box<Capture>, String>,
    ) -> Task<Message> {
        let active = self.active_tab().map(|t| t.repo.id());
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        tab.undo_busy = false;
        let Some(step) = tab.undo.next(redo).cloned() else {
            return Task::none();
        };
        let dirty = tab.snapshot.as_ref().is_some_and(|s| {
            s.status
                .entries
                .iter()
                .any(|e| !matches!(e, gf_core::model::StatusEntry::Ignored { .. }))
        });
        let now = match now {
            Ok(now) => now,
            Err(e) => {
                tab.undo_queue = None;
                self.flash(NoteKind::Error, format!("✗ {} — {e}", verb(redo)));
                return Task::none();
            }
        };
        // The dialogs act on the active tab.
        if active != Some(id) {
            tab.undo_queue = None;
            return Task::none();
        }
        let title = if redo { "Redo" } else { "Undo" };
        let what = format!("{title} \"{}\"?", step.label);
        let changed = step.expected(redo).differences(&now);
        if !changed.is_empty() {
            let mut body = vec![what, "Since then:".into()];
            body.extend(changed.iter().take(6).map(|c| format!("  • {c}")));
            if changed.len() > 6 {
                body.push(format!("  • and {} more", changed.len() - 6));
            }
            body.push(format!(
                "{title} anyway puts them back as they were {} it; those later changes are lost.",
                if redo { "after" } else { "before" }
            ));
            return self.confirm(Confirm::delete(
                format!("{title} anyway?"),
                body,
                format!("{title} anyway"),
                Purpose::Undo { redo, force: true },
            ));
        }
        if step.moves_head() {
            let target = step.target(redo);
            let branch = target
                .head
                .as_ref()
                .and_then(|h| h.branch.clone())
                .unwrap_or_else(|| "HEAD".into());
            let at = target
                .head
                .as_ref()
                .and_then(|h| h.oid.as_deref())
                .map_or_else(|| "no commit".to_owned(), |o| o.chars().take(7).collect());
            let keeps = match (step.mode, redo) {
                (HeadMode::Soft, false) => "the changes stay staged, nothing is lost",
                (HeadMode::Soft, true) => "the staged changes go back into it",
                (HeadMode::Mixed, _) => "the index follows, your files stay as they are",
                (HeadMode::Keep, _) => {
                    "files follow; uncommitted changes are kept (git refuses if they conflict)"
                }
            };
            let mut confirm = Confirm::change(
                "undo",
                title,
                vec![what, format!("`{branch}` goes to {at} — {keeps}.")],
                title,
                Purpose::Undo { redo, force: false },
            );
            if dirty && step.mode == HeadMode::Keep {
                confirm = confirm.check("stash my uncommitted changes first", false);
            }
            return self.confirm(confirm);
        }
        self.run_undo(redo, false, false)
    }

    /// Restores the next step's other side (asked or not). Unless
    /// `force`, the repository is checked against the step once more right
    /// before anything is written.
    pub(super) fn run_undo(&mut self, redo: bool, stash_first: bool, force: bool) -> Task<Message> {
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        let Some(step) = tab.undo.next(redo).cloned() else {
            return Task::none();
        };
        tab.undo_busy = true;
        let label = format!("{} {}", verb(redo), step.label);
        let stash_message = format!("gf: before {label}");
        self.op(label, OpKind::Undo { redo }, move |r| async move {
            if stash_first {
                r.stash_push(&StashOptions {
                    message: Some(stash_message),
                    include_untracked: true,
                    keep_index: false,
                })
                .await?;
            }
            let expected = (!force).then(|| step.expected(redo));
            r.restore(&step.scope, expected, step.target(redo), step.mode)
                .await
        })
    }

    /// An undo / redo finished: move the step, go on with the queue.
    pub(super) fn on_undo_done(&mut self, id: RepoId, redo: bool, ok: bool) -> Task<Message> {
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        tab.undo_busy = false;
        if !ok {
            tab.undo_queue = None;
            return Task::none();
        }
        let step = tab.undo.next(redo).cloned();
        tab.undo.applied(redo);
        tab.undo.save(&tab.repo.handle().git_dir);
        // An undone commit gives its message back to the commit box.
        // Redoing it takes the message back out (when still untouched).
        if let Some(message) = step.as_ref().and_then(|s| s.message.clone()) {
            if !redo && tab.commit.is_empty() {
                tab.commit.set_last_message(Some(message));
            } else if redo && tab.commit.holds(&message) {
                tab.commit.committed();
            }
        }
        let queue = tab.undo_queue.take();
        let label = step.map(|s| s.label).unwrap_or_default();
        let next = if redo { "⌘Z undo" } else { "⌘⇧Z redo" };
        self.flash(
            NoteKind::Info,
            format!(
                "{} {} · {next}",
                if redo { "↷ redid" } else { "↶ undid" },
                truncate(&label, 50)
            ),
        );
        match queue {
            Some(q) if q.redo == redo && self.active_tab().map(|t| t.repo.id()) == Some(id) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.undo_queue = (q.left > 1).then_some(Queue {
                        redo,
                        left: q.left - 1,
                    });
                }
                self.check_next(redo)
            }
            _ => Task::none(),
        }
    }

    /// Every changed path of the active tab (what stage all, stash or a
    /// hard reset touch).
    pub(super) fn changed_paths(&self) -> Vec<std::path::PathBuf> {
        self.active_tab()
            .and_then(|t| t.snapshot.as_ref())
            .map(|s| crate::undo::changed_paths(&s.status))
            .unwrap_or_default()
    }

    /// Which of `paths` are submodules of the active tab.
    pub(super) fn submodule_paths(&self, paths: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
        let Some(s) = self.active_tab().and_then(|t| t.snapshot.as_ref()) else {
            return Vec::new();
        };
        paths
            .iter()
            .filter(|p| s.submodules.iter().any(|m| &m.path == *p))
            .cloned()
            .collect()
    }

    // ------------------------------------------------------- history list

    pub(super) fn toggle_undo_list(&mut self) -> Task<Message> {
        if self.undo_list.take().is_some() {
            return self.restore_focus();
        }
        let has_steps = self
            .active_tab()
            .is_some_and(|t| !t.undo.undo.is_empty() || !t.undo.redo.is_empty());
        if !has_steps {
            self.flash(NoteKind::Info, "nothing to undo yet");
            return Task::none();
        }
        self.menu = None;
        self.tab_list = None;
        self.undo_list = Some(undo_list::State::default());
        Task::none()
    }

    pub(super) fn on_undo_list(&mut self, m: undo_list::Message) -> Task<Message> {
        match m {
            undo_list::Message::Toggle => self.toggle_undo_list(),
            undo_list::Message::Dismiss => {
                self.undo_list = None;
                self.restore_focus()
            }
            undo_list::Message::Pick(row) => {
                self.undo_list = None;
                let Some(tab) = self.active_tab() else {
                    return Task::none();
                };
                match undo_list::rows(&tab.undo).get(row) {
                    Some(r) => {
                        let (redo, n) = (r.redo, r.depth + 1);
                        self.undo_steps(redo, n)
                    }
                    None => Task::none(),
                }
            }
            undo_list::Message::Hover(row) => {
                if let Some(l) = self.undo_list.as_mut() {
                    l.selected = Some(row);
                }
                Task::none()
            }
        }
    }
}
