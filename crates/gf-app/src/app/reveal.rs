//! Finding a commit in the history (from blame): select it and scroll the
//! list to it, switching the view or loading more history only as far as
//! needed.
//!
//! 1. In the list shown: select and scroll.
//! 2. Otherwise `git rev-list` finds its row in the main history (HEAD, or
//!    every branch when that is on, else every branch if only that has it)
//!    without loading commit details, stopping as soon as it comes up.
//! 3. The main history is loaded down to that row (asking first past
//!    [`ASK_OVER`] commits), then the commit is selected and scrolled to.

use gf_core::model::{Oid, RepoId};
use gf_git::{LogOrder, LogQuery};
use iced::Task;

use super::dialogs::Purpose;
use super::{App, Message};
use crate::features::command_log::NoteKind;
use crate::features::dialog::{Dialog, Severity};
use crate::repo_tab::Parts;

/// Loading more commits than this asks first.
pub const ASK_OVER: usize = 10_000;

/// Where a commit is in the main history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Located {
    /// Row (0 = newest).
    pub row: usize,
    /// Only the all-branches history has it (HEAD's does not).
    pub needs_all: bool,
}

impl App {
    /// Selects `oid` in the history and scrolls to it (see the module docs).
    pub(super) fn reveal_commit(&mut self, oid: Oid) -> Task<Message> {
        let tokens = self.tokens.clone();
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        let Some(snap) = tab.snapshot.clone() else {
            return Task::none();
        };
        if let Some(task) = tab.history.reveal(&oid, &snap, &tokens) {
            self.expand_history();
            return task.map(Message::History);
        }
        let repo = tab.repo.clone();
        let id = repo.id();
        let all = tab.all_branches;
        Task::perform(
            {
                let oid = oid.clone();
                async move { locate(&repo, &oid, all).await.map_err(|e| e.to_string()) }
            },
            move |result| Message::CommitLocated {
                repo: id,
                oid: oid.clone(),
                result,
            },
        )
    }

    /// `rev-list` answered: load the history down to the commit (asking
    /// first when that is a lot), or say why it cannot be shown.
    pub(super) fn on_commit_located(
        &mut self,
        repo: RepoId,
        oid: Oid,
        result: Result<Option<Located>, String>,
    ) -> Task<Message> {
        if self.active_tab().map(|t| t.repo.id()) != Some(repo) {
            return Task::none();
        }
        match result {
            Err(e) => {
                self.flash(
                    NoteKind::Error,
                    format!("could not find {}: {e}", oid.short()),
                );
                Task::none()
            }
            Ok(None) => {
                self.flash(
                    NoteKind::Info,
                    format!(
                        "{} is on no branch or tag — the history cannot show it",
                        oid.short()
                    ),
                );
                Task::none()
            }
            Ok(Some(at)) if at.row + 1 > ASK_OVER => self.open_dialog(
                Dialog::confirm(
                    "Load history",
                    vec![
                        format!(
                            "{} is commit #{} of the history.",
                            oid.short(),
                            thousands(at.row + 1)
                        ),
                        "Load the history down to it? It may take a moment.".into(),
                    ],
                    "Load",
                    Purpose::RevealCommit { oid, at },
                )
                .severity(Severity::Normal),
            ),
            Ok(Some(at)) => self.show_located(oid, at),
        }
    }

    /// Switches to the main history (all branches when needed) with enough
    /// commits loaded, then reveals `oid`: now if it is already there, else
    /// when the longer history arrives (see `on_loaded`).
    pub(super) fn show_located(&mut self, oid: Oid, at: Located) -> Task<Message> {
        let tokens = self.tokens.clone();
        let Some(tab) = self.active_tab_mut() else {
            return Task::none();
        };
        let mut notes = Vec::new();
        if tab.history.revision().is_some() {
            tab.sidebar.clear_selection();
            tab.history.set_revision(None);
            notes.push("showing the full history".to_owned());
        }
        let switch_all = at.needs_all && !tab.all_branches;
        if switch_all {
            tab.all_branches = true;
            notes.push("all branches on: it is not in HEAD's history".to_owned());
        }
        let id = tab.repo.id();
        let shown = (!switch_all)
            .then(|| tab.snapshot.clone())
            .flatten()
            .and_then(|snap| tab.history.reveal(&oid, &snap, &tokens));
        let task = match shown {
            Some(task) => task.map(Message::History),
            None => {
                tab.history.reveal_after_load(oid.clone(), at.row + 1);
                self.request_parts(id, Parts::HISTORY)
            }
        };
        self.expand_history();
        if !notes.is_empty() {
            self.flash(
                NoteKind::Info,
                format!("{} — {}", oid.short(), notes.join("; ")),
            );
        }
        task
    }

    /// After a history load: reveals the commit waiting for it.
    pub(super) fn reveal_pending(&mut self, id: RepoId) -> Task<Message> {
        let tokens = self.tokens.clone();
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        let Some(snap) = tab.snapshot.clone() else {
            return Task::none();
        };
        let Some(oid) = tab.history.take_pending_reveal() else {
            return Task::none();
        };
        match tab.history.reveal(&oid, &snap, &tokens) {
            Some(task) => task.map(Message::History),
            None => {
                self.flash(
                    NoteKind::Info,
                    format!("{} is no longer in the history", oid.short()),
                );
                Task::none()
            }
        }
    }
}

/// Row of `oid` in the main history: HEAD's (or every branch's when `all`),
/// else every branch's when only that has it. `None`: in neither.
pub async fn locate(
    repo: &gf_git::Repository,
    oid: &Oid,
    all: bool,
) -> gf_core::Result<Option<Located>> {
    // The same walk as the history (`History::load`).
    let query = |all_refs| LogQuery {
        all_refs,
        order: LogOrder::Topo,
        ..LogQuery::default()
    };
    if let Some(row) = repo.log_position(&query(all), oid).await? {
        return Ok(Some(Located {
            row,
            needs_all: false,
        }));
    }
    if all {
        return Ok(None);
    }
    Ok(repo
        .log_position(&query(true), oid)
        .await?
        .map(|row| Located {
            row,
            needs_all: true,
        }))
}

/// `23,400`.
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::dialog;
    use crate::repo_tab::{RepoTab, Snapshot};
    use gf_test_support::TestRepo;
    use std::io::Write;

    /// `main` with `n` commits and a branch `side` (3 commits off main's
    /// 10th, or its last when shorter) that no other branch has, written by `git fast-import` (fast
    /// even for thousands). Returns main's ids, newest first, and side's.
    fn history_repo(n: usize) -> (TestRepo, Vec<Oid>, Vec<Oid>) {
        let repo = TestRepo::new();
        let mut stream = String::new();
        let commit = |s: &mut String, branch: &str, mark: usize, from: Option<usize>, msg: &str| {
            s.push_str(&format!(
                "commit refs/heads/{branch}\nmark :{mark}\ncommitter T <t@example.com> {} +0000\ndata {}\n{msg}\n",
                1_700_000_000 + mark,
                msg.len()
            ));
            if let Some(f) = from {
                s.push_str(&format!("from :{f}\n"));
            }
            s.push_str(&format!(
                "M 644 inline f.txt\ndata {}\n{mark}\n\n",
                mark.to_string().len()
            ));
        };
        for i in 1..=n {
            commit(
                &mut stream,
                "main",
                i,
                (i > 1).then(|| i - 1),
                &format!("main {i}"),
            );
        }
        for j in 0..3 {
            let mark = n + 1 + j;
            let from = if j == 0 { n.min(10) } else { mark - 1 };
            commit(&mut stream, "side", mark, Some(from), &format!("side {j}"));
        }
        let mut child = repo
            .command()
            .args(["fast-import", "--quiet"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("fast-import");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stream.as_bytes())
            .expect("write");
        assert!(child.wait().expect("wait").success());
        repo.git(&["reset", "-q", "--hard", "main"]);
        let ids = |rev: &str| -> Vec<Oid> {
            repo.git(&["rev-list", rev])
                .lines()
                .map(|l| l.parse().expect("oid"))
                .collect()
        };
        let main = ids("main");
        let side = ids("main..side");
        (repo, main, side)
    }

    async fn open(repo: &TestRepo) -> gf_git::Repository {
        let runner = gf_git::GitRunner::new()
            .with_cleared_env()
            .with_env(repo.env());
        gf_git::Repository::open(repo.path(), runner)
            .await
            .expect("open")
    }

    /// An app with `repo` open in one tab, its snapshot loaded like gf
    /// does (HEAD only unless `all`).
    async fn app_with(repo: &TestRepo, all: bool) -> (App, tempfile::TempDir) {
        let home = tempfile::tempdir().expect("home");
        let (mut app, _) = App::boot(
            gf_config::AppPaths::under(home.path()),
            gf_config::Settings::default(),
        );
        let r = open(repo).await;
        let mut tab = RepoTab::new(r, None);
        tab.all_branches = all;
        app.tabs.push(tab);
        app.active = 0;
        reload(&mut app).await;
        (app, home)
    }

    /// What the app's history load does, run here (tasks are not).
    async fn reload(app: &mut App) {
        let tab = app.active_tab_mut().expect("tab");
        let snap = Snapshot::load(
            tab.repo.clone(),
            tab.snapshot.clone(),
            crate::repo_tab::Parts::ALL,
            tab.all_branches,
            tab.history.head_limit(),
        )
        .await
        .expect("snapshot");
        let id = tab.repo.id();
        let _ = app.on_loaded(id, Ok(snap));
    }

    fn tab(app: &App) -> &RepoTab {
        app.active_tab().expect("tab")
    }

    fn shown_rows(app: &App) -> usize {
        tab(app)
            .snapshot
            .as_ref()
            .map_or(0, |s| s.history.commits.len())
    }

    fn flash(app: &App) -> String {
        app.flash
            .as_ref()
            .map(|f| f.text.clone())
            .unwrap_or_default()
    }

    /// Runs the lookup the app would run in the background, then hands
    /// the answer back like `Message::CommitLocated`.
    async fn find(app: &mut App, oid: &Oid) -> Result<Option<Located>, String> {
        let tab = tab(app);
        let (r, all) = (tab.repo.clone(), tab.all_branches);
        let result = locate(&r, oid, all).await.map_err(|e| e.to_string());
        let id = r.id();
        let _ = app.on_commit_located(id, oid.clone(), result.clone());
        result
    }

    #[tokio::test]
    async fn a_commit_already_listed_is_selected_at_once() {
        let (repo, main, _) = history_repo(30);
        let (mut app, _home) = app_with(&repo, false).await;
        let _ = app.reveal_commit(main[20].clone());
        assert_eq!(tab(&app).history.selected(), Some(&main[20]));
        assert!(flash(&app).is_empty(), "nothing switched, nothing to say");
    }

    #[tokio::test]
    async fn locate_finds_rows_like_the_history_lists_them() {
        let (repo, main, side) = history_repo(40);
        let r = open(&repo).await;
        // HEAD's history: main only, newest first.
        for (i, id) in main.iter().enumerate() {
            assert_eq!(
                locate(&r, id, false).await.expect("locate"),
                Some(Located {
                    row: i,
                    needs_all: false
                })
            );
        }
        // side is only in the all-branches history, at the same row there.
        let all = crate::repo_tab::History::load(&r, Vec::new(), true, 1000, &Default::default())
            .await
            .expect("history");
        for id in &side {
            let row = all
                .commits
                .iter()
                .position(|c| &c.id == id)
                .expect("listed");
            let found = locate(&r, id, false).await.expect("locate");
            assert_eq!(
                found,
                Some(Located {
                    row,
                    needs_all: true
                })
            );
            let found = locate(&r, id, true).await.expect("locate");
            assert_eq!(
                found,
                Some(Located {
                    row,
                    needs_all: false
                })
            );
        }
        let nowhere: Oid = "1".repeat(40).parse().expect("oid");
        assert_eq!(locate(&r, &nowhere, false).await.expect("locate"), None);
    }

    #[tokio::test]
    async fn a_commit_past_the_loaded_page_loads_down_to_it() {
        let (repo, main, _) = history_repo(2_500);
        let (mut app, _home) = app_with(&repo, false).await;
        assert_eq!(shown_rows(&app), crate::repo_tab::HISTORY_PAGE);
        let target = main[1_800].clone();
        assert!(
            tab(&app)
                .history
                .find(&target, tab(&app).snapshot.as_deref())
                .is_none()
        );

        assert_eq!(
            find(&mut app, &target).await,
            Ok(Some(Located {
                row: 1_800,
                needs_all: false
            }))
        );
        assert!(app.dialog.is_none(), "under the threshold: no question");
        assert!(tab(&app).history.head_limit() > 1_800);
        assert_eq!(
            tab(&app).history.selected(),
            None,
            "not yet: waits for the load"
        );

        reload(&mut app).await;
        assert!(shown_rows(&app) > 1_800);
        assert_eq!(tab(&app).history.selected(), Some(&target));
        assert_eq!(
            tab(&app)
                .snapshot
                .as_ref()
                .map(|s| s.history.commits[1_800].id.clone()),
            Some(target),
            "the row rev-list named is the commit"
        );
    }

    #[tokio::test]
    async fn a_commit_only_on_another_branch_turns_all_branches_on() {
        let (repo, _, side) = history_repo(30);
        let (mut app, _home) = app_with(&repo, false).await;
        let target = side[0].clone();
        let found = find(&mut app, &target)
            .await
            .expect("located")
            .expect("found");
        assert!(found.needs_all);
        assert!(tab(&app).all_branches, "switched on");
        assert!(flash(&app).contains("all branches on"), "{}", flash(&app));
        reload(&mut app).await;
        assert_eq!(tab(&app).history.selected(), Some(&target));
    }

    #[tokio::test]
    async fn a_branch_view_goes_back_to_the_full_history() {
        let (repo, main, _) = history_repo(30);
        let (mut app, _home) = app_with(&repo, true).await;
        // Showing `side` only: main's newest commit is not in it.
        {
            let tab = app.active_tab_mut().expect("tab");
            tab.history.set_revision(Some("refs/heads/side".into()));
            let r = tab.repo.clone();
            let side = crate::repo_tab::History::load(
                &r,
                vec!["refs/heads/side".into()],
                false,
                1000,
                &Default::default(),
            )
            .await
            .expect("side");
            tab.history
                .loaded("refs/heads/side", std::sync::Arc::new(side));
        }
        let target = main[0].clone();
        let _ = app.reveal_commit(target.clone());
        assert_eq!(tab(&app).history.selected(), None, "not in the branch view");
        let _ = find(&mut app, &target).await;
        assert_eq!(
            tab(&app).history.revision(),
            None,
            "back to the full history"
        );
        assert_eq!(
            tab(&app).history.selected(),
            Some(&target),
            "already loaded there: selected at once"
        );
        assert!(
            flash(&app).contains("showing the full history"),
            "{}",
            flash(&app)
        );
    }

    #[tokio::test]
    async fn a_far_commit_asks_before_loading() {
        let (repo, main, _) = history_repo(30);
        let (mut app, _home) = app_with(&repo, false).await;
        let id = tab(&app).repo.id();
        let far = Located {
            row: ASK_OVER + 2_399,
            needs_all: false,
        };
        let _ = app.on_commit_located(id, main[5].clone(), Ok(Some(far)));
        let purpose = app.dialog.as_ref().map(|d| d.purpose.clone());
        assert!(
            matches!(purpose, Some(Purpose::RevealCommit { at, .. }) if at == far),
            "{purpose:?}"
        );
        assert!(
            app.dialog
                .as_ref()
                .is_some_and(|d| d.body.iter().any(|l| l.contains("#12,400"))),
            "says how far"
        );
        assert_eq!(
            tab(&app).history.head_limit(),
            crate::repo_tab::HISTORY_PAGE,
            "nothing loaded yet"
        );
        // Cancel: nothing changes.
        let _ = app.on_dialog_outcome(dialog::Outcome::Cancelled {
            purpose: purpose.expect("purpose"),
        });
        assert!(app.dialog.is_none());
        assert_eq!(
            tab(&app).history.head_limit(),
            crate::repo_tab::HISTORY_PAGE
        );
    }

    #[tokio::test]
    async fn a_commit_on_no_branch_is_explained() {
        let (repo, _, _) = history_repo(5);
        let (mut app, _home) = app_with(&repo, false).await;
        let nowhere: Oid = "2".repeat(40).parse().expect("oid");
        assert_eq!(find(&mut app, &nowhere).await, Ok(None));
        assert!(
            flash(&app).contains("on no branch or tag"),
            "{}",
            flash(&app)
        );
        assert!(!tab(&app).all_branches, "nothing switched");
    }

    #[tokio::test]
    async fn a_commit_gone_by_the_time_the_history_arrives_is_reported() {
        let (repo, main, _) = history_repo(5);
        let (mut app, _home) = app_with(&repo, false).await;
        let gone: Oid = "3".repeat(40).parse().expect("oid");
        app.active_tab_mut()
            .expect("tab")
            .history
            .reveal_after_load(gone, 3);
        reload(&mut app).await;
        assert!(
            flash(&app).contains("no longer in the history"),
            "{}",
            flash(&app)
        );
        assert_eq!(tab(&app).history.selected(), None);
        // The next load does not report it again.
        app.flash = None;
        reload(&mut app).await;
        assert!(flash(&app).is_empty());
        let _ = main;
    }

    #[tokio::test]
    async fn clicking_a_blame_line_keeps_blame_open() {
        let (repo, main, _) = history_repo(10);
        let (mut app, _home) = app_with(&repo, false).await;
        app.active_tab_mut().expect("tab").blame = Some(
            crate::features::blame_view::State::loading("f.txt".into(), None),
        );
        let _ = app.on_blame_message(crate::features::blame_view::Message::Select(
            main[3].clone(),
        ));
        assert!(tab(&app).blame.is_some(), "still open");
        assert_eq!(tab(&app).history.selected(), Some(&main[3]));
        let _ = app.on_blame_message(crate::features::blame_view::Message::Close);
        assert!(tab(&app).blame.is_none(), "close still closes");
    }

    #[tokio::test]
    async fn clicking_a_history_row_closes_blame_and_shows_the_commit() {
        let (repo, main, _) = history_repo(10);
        let (mut app, _home) = app_with(&repo, false).await;
        app.active_tab_mut().expect("tab").blame = Some(
            crate::features::blame_view::State::loading("f.txt".into(), None),
        );
        let _ = app.update(Message::History(crate::features::history::Message::Select(
            main[2].clone(),
        )));
        assert!(tab(&app).blame.is_none(), "blame gave way");
        assert_eq!(tab(&app).history.selected(), Some(&main[2]));
        assert!(
            app.panes().contains(&crate::nav::Pane::Files),
            "files are back"
        );
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(23_400), "23,400");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
