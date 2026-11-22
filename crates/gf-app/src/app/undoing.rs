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
//!
//! Several steps at once (picked in the history list) are asked once, for
//! all of them ([`App::confirm_batch`]); the rest then run one after the
//! other without asking, each still guarded, and the batch stops at the
//! first one that fails.

use gf_core::model::RepoId;
use gf_git::{Capture, HeadMode, StashOptions};
use iced::Task;

use super::dialogs::{Confirm, Purpose};
use super::{App, Message, OpKind};
use crate::features::command_log::NoteKind;
use crate::features::undo_list;
use crate::undo::{Queue, Step, batch_move};
use crate::widgets::format::truncate;

/// Points the repository's undo ref at every file copy `tab`'s history
/// can still write back (dropping the ones it no longer can).
pub(super) fn pin_undo_copies(tab: &crate::repo_tab::RepoTab) -> Task<Message> {
    let blobs = tab.undo.saved_blobs();
    let repo = tab.repo.clone();
    Task::perform(
        async move {
            repo.pin_saved_files(&blobs)
                .await
                .map_err(|e| e.to_string())
        },
        Message::UndoPinned,
    )
}

fn verb(redo: bool) -> &'static str {
    if redo { "redo" } else { "undo" }
}

/// What happens to the files when a step moves HEAD back (or forward).
fn keeps(mode: HeadMode, redo: bool) -> &'static str {
    match (mode, redo) {
        (HeadMode::Soft, false) => "the changes stay staged, nothing is lost",
        (HeadMode::Soft, true) => "the staged changes go back into it",
        (HeadMode::Mixed, _) => "the index follows, your files stay as they are",
        (HeadMode::Keep, _) => {
            "files follow; uncommitted changes are kept (git refuses if they conflict)"
        }
    }
}

/// `` `main` goes to a1b2c3d `` for where `redo` (or undo) of `step` puts
/// HEAD.
fn head_line(step: &Step, redo: bool) -> String {
    let target = step.target(redo).head.as_ref();
    let branch = target
        .and_then(|h| h.branch.clone())
        .unwrap_or_else(|| "HEAD".into());
    let at = target
        .and_then(|h| h.oid.as_deref())
        .map_or_else(|| "no commit".to_owned(), |o| o.chars().take(7).collect());
    format!("`{branch}` goes to {at}")
}

/// The "Since then:" lines for what an undo would overwrite.
fn changed_lines(changed: &[String]) -> Vec<String> {
    let mut body = vec!["Since then:".to_owned()];
    body.extend(changed.iter().take(6).map(|c| format!("  • {c}")));
    if changed.len() > 6 {
        body.push(format!("  • and {} more", changed.len() - 6));
    }
    body
}

/// The confirmation line for `published` commits that leave `branch`
/// and are already on its upstream.
fn published_note(published: usize, branch: Option<&str>) -> Option<String> {
    let branch = branch?;
    let commits = if published == 1 {
        "1 commit".to_owned()
    } else {
        format!("{published} commits")
    };
    (published > 0).then(|| {
        format!(
            "⚠ {commits} leaving `{branch}` already pushed: they stay on the remote, \
             so the next pull brings them back and a push needs force."
        )
    })
}

impl App {
    /// A finished operation's step: on the undo stack, saved, and its file
    /// copies kept from `git gc`.
    pub(super) fn record_step(&mut self, id: RepoId, step: Step) -> Task<Message> {
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        tab.undo.push(step);
        tab.undo.save(&tab.repo.handle().git_dir);
        pin_undo_copies(tab)
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
        let in_progress = tab.snapshot.as_ref().is_some_and(|s| !s.state.is_clean());
        if n > 1 {
            // Asked once for the whole batch: refuse it whole rather than
            // stop half way at a step that can't run now.
            let steps = tab.undo.upcoming(redo, n);
            if in_progress && let Some(s) = steps.iter().find(|s| s.moves_head()) {
                let label = truncate(&s.label, 40);
                self.flash(
                    NoteKind::Error,
                    format!(
                        "can't {} up to here: \"{label}\" moves HEAD in the middle of a \
                         merge or rebase — continue or abort it first",
                        verb(redo)
                    ),
                );
                return Task::none();
            }
            tab.undo_queue = Some(Queue::new(redo, steps.len()));
        } else {
            tab.undo_queue = None;
        }
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
        // A batch about to be asked: the branch moves over all its steps.
        let steps = match &tab.undo_queue {
            Some(q) if !q.confirmed => tab.undo.upcoming(redo, q.total),
            _ => vec![step.clone()],
        };
        let repo = tab.repo.clone();
        let id = repo.id();
        Task::perform(
            async move {
                let now = repo
                    .capture(&step.scope)
                    .await
                    .map(Box::new)
                    .map_err(|e| e.to_string());
                let published = match batch_move(&steps, redo) {
                    // Best effort: an unreadable upstream only skips the
                    // warning.
                    Some((branch, from, to)) => {
                        repo.published_leaving(branch, from, to).await.unwrap_or(0)
                    }
                    None => 0,
                };
                (now, published)
            },
            move |(now, published)| Message::UndoChecked {
                repo: id,
                redo,
                now,
                published,
            },
        )
    }

    pub(super) fn on_undo_checked(
        &mut self,
        id: RepoId,
        redo: bool,
        now: Result<Box<Capture>, String>,
        published: usize,
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
            // This step did not run either.
            let left = tab.undo_queue.take().map(|q| q.left + 1);
            self.queue_stopped(redo, left);
            return Task::none();
        }
        if let Some(q) = tab.undo_queue.as_ref().filter(|q| !q.confirmed) {
            let steps = tab.undo.upcoming(redo, q.total);
            return self.confirm_batch(redo, &steps, &now, published, dirty);
        }
        let title = if redo { "Redo" } else { "Undo" };
        let what = format!("{title} \"{}\"?", step.label);
        let pushed = published_note(published, step.branch_move(redo).map(|(b, ..)| b));
        let changed = step.expected(redo).differences(&now);
        if !changed.is_empty() {
            let mut body = vec![what];
            body.extend(changed_lines(&changed));
            body.extend(pushed);
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
            let mut body = vec![
                what,
                format!("{} — {}.", head_line(&step, redo), keeps(step.mode, redo)),
            ];
            let purpose = Purpose::Undo { redo, force: false };
            // Taking pushed commits off the branch is always asked, even
            // with the undo confirmation turned off.
            let mut confirm = match pushed {
                Some(note) => {
                    body.push(note);
                    Confirm::delete(format!("{title} pushed commits?"), body, title, purpose)
                }
                None => Confirm::change("undo", title, body, title, purpose),
            };
            if dirty && step.mode == HeadMode::Keep {
                confirm = confirm.check("stash my uncommitted changes first", false);
            }
            return self.confirm(confirm);
        }
        self.run_undo(redo, false, false)
    }

    /// One confirmation for the next `steps` (a batch from the history
    /// list), or none when they only touch what is still as they left it
    /// and none moves HEAD. `now` is the repository as the first step
    /// covers it.
    fn confirm_batch(
        &mut self,
        redo: bool,
        steps: &[Step],
        now: &Capture,
        published: usize,
        dirty: bool,
    ) -> Task<Message> {
        let Some(first) = steps.first() else {
            return Task::none();
        };
        let n = steps.len();
        let changed = first.expected(redo).differences(now);
        let moving: Vec<&Step> = steps.iter().filter(|s| s.moves_head()).collect();
        let pushed = published_note(published, batch_move(steps, redo).map(|(b, ..)| b));
        if changed.is_empty() && moving.is_empty() {
            return self.run_undo(redo, false, false);
        }
        let title = if redo { "Redo" } else { "Undo" };
        let (order, icon) = if redo {
            ("oldest first", "↷")
        } else {
            ("newest first", "↶")
        };
        let mut body = vec![format!("{title} these {n} steps ({order})?")];
        body.extend(
            steps
                .iter()
                .take(6)
                .map(|s| format!("  {icon} {}", truncate(&s.label, 60))),
        );
        if n > 6 {
            body.push(format!("  … and {} more", n - 6));
        }
        if let Some(last) = moving.last() {
            body.push(format!(
                "In the end {} — {}.",
                head_line(last, redo),
                keeps(last.mode, redo)
            ));
        }
        if !changed.is_empty() {
            body.extend(changed_lines(&changed));
            body.push(format!(
                "The first step puts them back as they were {} it; those later changes are lost.",
                if redo { "after" } else { "before" }
            ));
        }
        body.push("Each step is still checked as it runs; the rest stop at one that fails.".into());
        let force = !changed.is_empty();
        let purpose = Purpose::Undo { redo, force };
        let button = format!("{title} {n}");
        // Overwriting later changes or taking pushed commits off the
        // branch is always asked.
        let mut confirm = match pushed {
            Some(note) => {
                body.push(note);
                Confirm::delete(
                    format!("{title} {n} steps with pushed commits?"),
                    body,
                    button,
                    purpose,
                )
            }
            None if force => {
                Confirm::delete(format!("{title} {n} steps anyway?"), body, button, purpose)
            }
            None => Confirm::change("undo", format!("{title} {n} steps"), body, button, purpose),
        };
        if dirty && moving.iter().any(|s| s.mode == HeadMode::Keep) {
            confirm = confirm.check("stash my uncommitted changes first", false);
        }
        self.confirm(confirm)
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
        let mut label = format!("{} {}", verb(redo), step.label);
        let stash_message = format!("gf: before {label}");
        // Running means the batch was confirmed: the rest go without asking.
        // Its steps say where they are, so a failure says where it stopped.
        if let Some(q) = tab.undo_queue.as_mut() {
            q.confirmed = true;
            if stash_first {
                q.stashed = Some(stash_message.clone());
            }
            label = format!("{} {}/{}: {}", verb(redo), q.done(), q.total, step.label);
        }
        self.op(label, OpKind::Undo { redo }, move |r| async move {
            if stash_first {
                r.stash_push(&StashOptions {
                    message: Some(stash_message.clone()),
                    include_untracked: true,
                    keep_index: false,
                    paths: Vec::new(),
                })
                .await?;
            }
            let expected = (!force).then(|| step.expected(redo));
            let restored = r
                .restore(&step.scope, expected, step.target(redo), step.mode)
                .await;
            // A failed restore puts back what it wrote; the changes stashed
            // for it come back too, so "nothing was changed" holds.
            match restored {
                Err(e) if stash_first => Err(match r.stash_pop(0).await {
                    Ok(()) => e,
                    Err(p) => gf_core::GfError::Other(format!(
                        "{e}; your changes stay in the stash \"{stash_message}\" ({p})"
                    )),
                }),
                other => other,
            }
        })
    }

    /// An undo / redo finished: move the step, go on with the queue.
    pub(super) fn on_undo_done(&mut self, id: RepoId, redo: bool, ok: bool) -> Task<Message> {
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        tab.undo_busy = false;
        if !ok {
            // The failed step changed nothing; earlier ones of the batch
            // stay done (each is a step of its own to redo / undo).
            // The error itself is reported by the operation (its label
            // says which step of the batch failed).
            if let Some(q) = tab.undo_queue.take() {
                let done = q.done() - 1;
                let mut lines = vec![format!(
                    "{done} done, {} not run; each done step can be redone or undone on its own",
                    q.total - done
                )];
                if let Some(stash) = q.stashed.filter(|_| done > 0) {
                    lines.push(format!(
                        "your uncommitted changes are in the stash \"{stash}\""
                    ));
                }
                self.log.note(
                    NoteKind::Info,
                    format!("{} stopped after {done} of {} steps", verb(redo), q.total),
                    lines,
                );
            }
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
        let queue = tab.undo_queue.take().filter(|q| q.redo == redo);
        let label = step.map(|s| s.label).unwrap_or_default();
        let next = if redo { "⌘Z undo" } else { "⌘⇧Z redo" };
        let did = if redo { "↷ redid" } else { "↶ undid" };
        let Some(mut q) = queue else {
            self.flash(
                NoteKind::Info,
                format!("{did} {} · {next}", truncate(&label, 50)),
            );
            return Task::none();
        };
        if q.left == 0 {
            self.flash(NoteKind::Info, format!("{did} {} steps · {next}", q.total));
            return Task::none();
        }
        if self.active_tab().map(|t| t.repo.id()) != Some(id) {
            self.queue_stopped(redo, Some(q.left));
            return Task::none();
        }
        self.flash(
            NoteKind::Info,
            format!("{did} {} · {}/{}", truncate(&label, 50), q.done(), q.total),
        );
        q.left -= 1;
        if let Some(tab) = self.tab_mut(id) {
            tab.undo_queue = Some(q);
        }
        // Confirmed already: guarded by `restore` itself, no new question.
        self.run_undo(redo, false, false)
    }

    /// Several steps were asked for, but another tab was opened before
    /// they all ran: say how many were left, so it does not stop silently.
    fn queue_stopped(&mut self, redo: bool, left: Option<usize>) {
        if let Some(left) = left.filter(|&n| n > 0) {
            let steps = if left == 1 { "step" } else { "steps" };
            self.flash(
                NoteKind::Info,
                format!(
                    "{} stopped: another tab was opened — {left} {steps} left",
                    verb(redo)
                ),
            );
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
