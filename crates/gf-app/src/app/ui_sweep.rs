//! Clicks every button gf shows and checks that each one does something.
//!
//! The real view is laid out headless (`iced_test`), every visible piece of
//! text is clicked (left and right button) and the messages it produces
//! run through [`App::update`] with their tasks, like the runtime would.
//! A click that produces a message but changes nothing (app state, the
//! repository, focus, clipboard) is a button that "silently does nothing".
//!
//! Screens with their own buttons (dialogs, menus, the open window, ...)
//! are reached by clicking from the base screens, then swept the same way.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use gf_test_support::TestRepo;
use iced::futures::StreamExt;
use iced_test::core::{Point, Size, mouse};
use iced_test::runtime::{self, Action};
use iced_test::selector::Candidate;

use super::*;
use crate::widgets::file_tree;

const WINDOW: Size = Size::new(1280.0, 820.0);

/// Longest a single task may take before it counts as still running.
const TASK_TIMEOUT: Duration = Duration::from_secs(20);

/// A repository with something in every list: commits on two branches, a
/// tag, a stash, a remote with a pushed branch, and staged, modified and
/// untracked files.
pub(super) struct Fixture {
    pub repo: TestRepo,
}

impl Fixture {
    /// A fresh copy of the fixture (built once, then copied: tests click
    /// hundreds of times, each from a fresh repository).
    pub fn new() -> Self {
        static TEMPLATE: std::sync::OnceLock<Fixture> = std::sync::OnceLock::new();
        let template = TEMPLATE.get_or_init(Self::build);
        Self {
            repo: template.repo.duplicate(),
        }
    }

    fn build() -> Self {
        let repo = TestRepo::new();
        // The remote lives inside `.git` (a relative url), so one copy of
        // the folder copies both; no hook samples to copy either.
        let _ = std::fs::remove_dir_all(repo.path().join(".git/hooks"));
        let remote = repo.path().join(".git/remote.git");
        let out = repo
            .command()
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(&remote)
            .output()
            .expect("git init");
        assert!(out.status.success(), "{out:?}");
        let _ = std::fs::remove_dir_all(remote.join("hooks"));
        repo.write("a.txt", "one\ntwo\nthree\n");
        repo.write("src/lib.rs", "fn main() {}\n");
        repo.commit_all("first");
        repo.write("a.txt", "one\ntwo\nthree\nfour\n");
        repo.commit_all("second");
        repo.git(&["tag", "v1"]);
        repo.git(&["branch", "feature"]);
        repo.write("src/lib.rs", "fn main() {\n    println!(\"hi\");\n}\n");
        repo.commit_all("third");
        repo.git(&["remote", "add", "origin", "./.git/remote.git"]);
        repo.git(&["push", "-q", "-u", "origin", "main", "feature"]);
        repo.write("a.txt", "stashed\n");
        repo.git(&["stash", "push", "-q", "-m", "wip"]);
        repo.write("a.txt", "one\ntwo\nchanged\nfour\n");
        repo.write("b.txt", "new staged\n");
        repo.git(&["add", "b.txt"]);
        repo.write("c.txt", "untracked\n");
        Self { repo }
    }
}

fn advance_fixture_remote(repo: &TestRepo) -> String {
    let tip = repo.git(&["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "incoming"]);
    // A push by URL leaves the configured remote-tracking ref untouched.
    repo.git(&["push", ".git/remote.git", &format!("{tip}:refs/heads/main")]);
    tip
}

#[tokio::test]
async fn sync_pull_runs_quietly_when_the_remote_can_fast_forward() {
    let mut incoming = String::new();
    let mut ui = Ui::open_with(|repo| incoming = advance_fixture_remote(repo)).await;
    assert_eq!(
        ui.app
            .active_tab()
            .and_then(|tab| tab.snapshot.as_ref())
            .expect("fixture snapshot")
            .status
            .head
            .behind,
        0,
        "the incoming commit has not been fetched",
    );
    let _ = ui.send(Message::Toolbar(toolbar::Message::Pull)).await;
    assert!(ui.app.dialog.is_none(), "{:?}", ui.app.dialog);
    assert_eq!(ui.fixture.repo.git(&["rev-parse", "HEAD"]), incoming);
}

#[tokio::test]
async fn sync_pull_asks_when_the_remote_diverged_since_fetch() {
    let mut ui = Ui::open_with(|repo| {
        advance_fixture_remote(repo);
        repo.git(&["commit", "--allow-empty", "-m", "outgoing"]);
        repo.git(&["config", "pull.rebase", "true"]);
    })
    .await;
    let before = ui.fixture.repo.git(&["rev-parse", "HEAD"]);
    let _ = ui.send(Message::Toolbar(toolbar::Message::Pull)).await;
    assert_eq!(
        ui.app.dialog.as_ref().map(|d| d.title.as_str()),
        Some("Branches have diverged")
    );
    assert_eq!(ui.fixture.repo.git(&["rev-parse", "HEAD"]), before);
}

#[tokio::test]
async fn sync_pull_offers_autostash_only_when_local_files_overlap() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["switch", "-c", "incoming"]);
        repo.write("a.txt", "remote edit\n");
        repo.git(&["commit", "-m", "incoming", "--", "a.txt"]);
        repo.git(&["push", ".git/remote.git", "incoming:main"]);
        repo.git(&["switch", "main"]);
        repo.write("a.txt", "local edit\n");
    })
    .await;
    let before = ui.fixture.repo.git(&["rev-parse", "HEAD"]);
    let _ = ui.send(Message::Toolbar(toolbar::Message::Pull)).await;
    let dialog = ui
        .app
        .dialog
        .as_ref()
        .expect("overlapping changes are reviewed");
    assert_eq!(dialog.title, "Local changes overlap the pull");
    assert_eq!(ui.fixture.repo.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(
        std::fs::read_to_string(ui.fixture.repo.path().join("a.txt")).expect("local file"),
        "local edit\n"
    );
}

#[tokio::test]
async fn sync_push_runs_quietly_for_a_fast_forward() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["commit", "--allow-empty", "-m", "outgoing"]);
    })
    .await;
    let _ = ui.send(Message::Toolbar(toolbar::Message::Push)).await;
    assert!(ui.app.dialog.is_none(), "{:?}", ui.app.dialog);
    let repo = &ui.fixture.repo;
    assert_eq!(
        repo.git(&["--git-dir=.git/remote.git", "rev-parse", "main"]),
        repo.git(&["rev-parse", "HEAD"])
    );
}

#[tokio::test]
async fn history_marks_unpushed_commits_until_push() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["commit", "--allow-empty", "-m", "outgoing"]);
    })
    .await;
    let outgoing: gf_core::model::Oid = ui.fixture.repo.head().parse().expect("oid");
    let arrows: Vec<_> = ui
        .targets()
        .into_iter()
        .filter(|target| target.label == "⇡")
        .collect();
    assert_eq!(arrows.len(), 1, "only the unpublished commit is marked");

    let _ = ui.click_at(&arrows[0], mouse::Button::Left).await;
    assert_eq!(
        ui.app.active_tab().expect("tab").history.selected(),
        Some(&outgoing),
        "the row remains selectable through the marker"
    );

    let _ = ui.send(Message::Toolbar(toolbar::Message::Push)).await;
    let tab = ui.app.active_tab().expect("tab");
    assert!(
        tab.snapshot
            .as_ref()
            .expect("snapshot")
            .unpushed_commits
            .is_empty()
    );
    assert_eq!(
        tab.history.selected(),
        Some(&outgoing),
        "push keeps selection"
    );
    assert!(
        !ui.targets().iter().any(|target| target.label == "⇡"),
        "the marker disappears after push"
    );
}

#[tokio::test]
async fn history_file_search_uses_the_head_or_selected_branch_tree() {
    let mut ui = Ui::open_with(|repo| {
        repo.write("branch-only.txt", "branch version\n");
        repo.commit_all("branch file");
        repo.git(&["branch", "search-branch"]);
        repo.write("head-only.txt", "head version\n");
        repo.commit_all("head file");
    })
    .await;

    let _ = ui.click("Find ⌘F").await;
    let _ = ui
        .send(Message::History(history::Message::Search(
            history::search::Message::QueryChanged("head-only".into()),
        )))
        .await;
    let history = &ui.app.active_tab().expect("tab").history;
    assert_eq!(
        history.file_search_state().label_for_hit(0),
        Some("head-only.txt")
    );
    assert_eq!(history.file_search_state().label_for_hit(1), None);
    let _ = ui
        .send(Message::History(history::Message::Search(
            history::search::Message::Dismiss,
        )))
        .await;

    let _ = ui
        .send(Message::Sidebar(crate::features::sidebar::Message::Select(
            crate::features::sidebar::Selection::Ref("refs/heads/search-branch".into()),
        )))
        .await;
    assert_eq!(
        ui.app.active_tab().expect("tab").history.revision(),
        Some("refs/heads/search-branch")
    );
    let _ = ui.click("Find ⌘F").await;
    let _ = ui
        .send(Message::History(history::Message::Search(
            history::search::Message::QueryChanged("head-only".into()),
        )))
        .await;
    let history = &ui.app.active_tab().expect("tab").history;
    assert_eq!(history.file_search_state().label_for_hit(0), None);
    assert_eq!(
        history.file_search_state().status_text(),
        None,
        "an empty filtered result is ready, not a loading state"
    );
    assert_eq!(history.search_state().expect("search").selected(), None);

    let _ = ui
        .send(Message::History(history::Message::Search(
            history::search::Message::QueryChanged("branch-only".into()),
        )))
        .await;
    assert_eq!(
        ui.app
            .active_tab()
            .expect("tab")
            .history
            .file_search_state()
            .label_for_hit(0),
        Some("branch-only.txt")
    );
    let _ = ui.send(esc()).await;
    assert!(
        !ui.app
            .active_tab()
            .expect("tab")
            .history
            .file_search_is_open()
    );
    assert_eq!(
        ui.app.active_tab().expect("tab").history.revision(),
        Some("refs/heads/search-branch"),
        "esc only closes the search"
    );
}

fn esc() -> Message {
    Message::KeyInput(focus::KeyInput {
        shortcut: Some(Shortcut::Escape),
        menu: None,
        nav: None,
    })
}

fn enter() -> Message {
    Message::KeyInput(focus::KeyInput {
        shortcut: Some(Shortcut::Enter),
        menu: None,
        nav: Some(crate::nav::Nav::plain(crate::nav::Key::Enter)),
    })
}

fn search_query(query: &str) -> Message {
    Message::History(history::Message::Search(
        history::search::Message::QueryChanged(query.into()),
    ))
}

fn loaded_summaries(ui: &Ui) -> Vec<String> {
    ui.app
        .active_tab()
        .expect("tab")
        .history
        .loaded_commits()
        .expect("file history loaded")
        .iter()
        .map(|c| c.summary.clone())
        .collect()
}

fn renamed_file_repo(repo: &TestRepo) {
    repo.write("src/old.rs", "fn old() {}\n");
    repo.commit_all("add old");
    repo.git(&["mv", "src/old.rs", "src/core.rs"]);
    repo.commit_all("rename core");
    repo.write("src/core.rs", "fn core() {}\n");
    repo.write("docs/notes.md", "notes\n");
    repo.commit_all("edit core and docs");
    repo.write("tests/core.rs", "#[test] fn t() {}\n");
    repo.commit_all("tests core");
    repo.write("unrelated.txt", "x\n");
    repo.commit_all("unrelated");
}

#[tokio::test]
async fn history_file_search_opens_the_picked_files_history_and_restores_the_scope() {
    let mut ui = Ui::open_with(renamed_file_repo).await;
    let head = ui.fixture.repo.head();

    let _ = ui.click("Find ⌘F").await;
    let _ = ui.send(search_query("src/core")).await;
    let state = ui
        .app
        .active_tab()
        .expect("tab")
        .history
        .file_search_state();
    assert_eq!(state.label_for_hit(0), Some("src/core.rs"));
    let _ = ui.send(enter()).await;

    let tab = ui.app.active_tab().expect("tab");
    assert!(!tab.history.file_search_is_open());
    assert_eq!(
        tab.history.searched_file(),
        Some(std::path::Path::new("src/core.rs"))
    );
    let (_, start) = tab.history.file_revision.clone().expect("file history");
    assert_eq!(start.map(|o| o.to_string()), Some(head));
    assert!(ui.shows("showing: HEAD · src/core.rs"));
    assert_eq!(
        loaded_summaries(&ui),
        ["edit core and docs", "rename core", "add old"],
        "only commits that changed the file, following its rename"
    );

    // docs/notes.md sorts first in the diff; the searched file opens.
    let edit = ui
        .app
        .active_tab()
        .expect("tab")
        .history
        .loaded_commits()
        .expect("loaded")[0]
        .id
        .clone();
    let _ = ui
        .send(Message::History(history::Message::Select(edit.clone())))
        .await;
    let diff = ui
        .app
        .active_tab()
        .expect("tab")
        .diff(diff_view::Role::Commit);
    assert_eq!(diff.current_path(), Some(PathBuf::from("src/core.rs")));

    // Already open: picks the file again without reloading.
    let _ = ui
        .send(Message::CommitDiff(diff_view::Message::SelectFile(0)))
        .await;
    let _ = ui
        .send(Message::History(history::Message::Select(edit)))
        .await;
    let diff = ui
        .app
        .active_tab()
        .expect("tab")
        .diff(diff_view::Role::Commit);
    assert_eq!(diff.current_path(), Some(PathBuf::from("src/core.rs")));

    let stale = ui.app.active_tab().expect("tab").history_request;
    let label = ui
        .app
        .active_tab()
        .expect("tab")
        .history
        .revision()
        .expect("file label")
        .to_owned();
    // Esc goes back, like `×`.
    let _ = ui.send(Message::Key(Shortcut::Escape)).await;
    let tab = ui.app.active_tab().expect("tab");
    assert_eq!(tab.history.revision(), None);
    assert_eq!(tab.history.searched_file(), None);
    let id = tab.repo.id();
    let _ = ui
        .send(Message::HistoryLoaded(id, stale, label, Err("late".into())))
        .await;
    assert_eq!(ui.app.active_tab().expect("tab").history.revision(), None);
    assert!(ui.shows("Find ⌘F"));
}

#[tokio::test]
async fn history_file_search_scrolls_the_files_list_to_the_picked_file_once() {
    let mut ui = Ui::open_with(|repo| {
        for i in 0..40 {
            repo.write(format!("dir{}/file{i:02}.txt", i % 4), "x\n");
        }
        repo.commit_all("many files");
        repo.write("dir3/file39.txt", "y\n");
        repo.commit_all("touch the last file");
    })
    .await;
    let _ = ui.click("Find ⌘F").await;
    let _ = ui.send(search_query("dir3/file39")).await;
    let _ = ui.send(enter()).await;
    let first = ui.app.active_tab().expect("tab").history.loaded_commits();
    assert_eq!(first.expect("loaded").len(), 2);
    let commit = first.expect("loaded")[1].id.clone();

    // Opening the older commit loads its diff: the picked file is chosen
    // and the list is scrolled to it exactly once.
    let _ = ui
        .send(Message::History(history::Message::Select(commit.clone())))
        .await;
    let diff = ui
        .app
        .active_tab()
        .expect("tab")
        .diff(diff_view::Role::Commit);
    assert_eq!(diff.current_path(), Some(PathBuf::from("dir3/file39.txt")));
    let diff = ui
        .app
        .active_tab_mut()
        .expect("tab")
        .diff_mut(diff_view::Role::Commit);
    assert!(!diff.take_reveal(), "the scroll request was consumed");

    // Already open: picking it again scrolls again, without a reload.
    let _ = ui
        .send(Message::History(history::Message::Select(commit)))
        .await;
    let diff = ui
        .app
        .active_tab_mut()
        .expect("tab")
        .diff_mut(diff_view::Role::Commit);
    assert!(!diff.take_reveal(), "nothing is left pending");
}

#[tokio::test]
async fn history_file_search_on_a_branch_keeps_its_tip_and_returns_to_it() {
    let mut ui = Ui::open_with(|repo| {
        renamed_file_repo(repo);
        repo.git(&["branch", "pinned"]);
    })
    .await;
    let branch_tip = ui.fixture.repo.head();
    let _ = ui
        .send(Message::Sidebar(crate::features::sidebar::Message::Select(
            crate::features::sidebar::Selection::Ref("refs/heads/pinned".into()),
        )))
        .await;
    let _ = ui.click("Find ⌘F").await;
    let _ = ui.send(search_query("src/core")).await;
    let _ = ui
        .send(Message::History(history::Message::Search(
            history::search::Message::Activate(0),
        )))
        .await;
    assert!(ui.shows("showing: pinned · src/core.rs"));
    let (_, start) = ui
        .app
        .active_tab()
        .expect("tab")
        .history
        .file_revision
        .clone()
        .expect("file history");
    assert_eq!(start.map(|o| o.to_string()), Some(branch_tip));

    let _ = ui
        .send(Message::History(history::Message::RestoreFileHistory))
        .await;
    let tab = ui.app.active_tab().expect("tab");
    assert_eq!(tab.history.revision(), Some("refs/heads/pinned"));
    assert!(
        tab.history.loaded_commits().is_some(),
        "branch history reloaded"
    );
}

#[tokio::test]
async fn history_file_search_dismiss_and_stale_rows_do_not_open_a_file() {
    let mut ui = Ui::open_with(renamed_file_repo).await;
    let _ = ui.click("Find ⌘F").await;
    let _ = ui.send(search_query("zzz-no-such-file")).await;
    assert!(ui.shows("no files match"));
    let _ = ui.send(enter()).await;
    let _ = ui
        .send(Message::History(history::Message::Search(
            history::search::Message::Activate(0),
        )))
        .await;
    let tab = ui.app.active_tab().expect("tab");
    assert!(tab.history.file_search_is_open());
    assert_eq!(tab.history.revision(), None);
    let _ = ui.send(esc()).await;
    assert!(
        !ui.app
            .active_tab()
            .expect("tab")
            .history
            .file_search_is_open()
    );
    assert_eq!(ui.app.active_tab().expect("tab").history.revision(), None);
}

fn command_f() -> Message {
    Message::KeyInput(focus::KeyInput {
        shortcut: Some(Shortcut::FindInFile),
        menu: None,
        nav: None,
    })
}

#[tokio::test]
async fn command_f_opens_the_history_find_from_anywhere_in_the_main_window() {
    let mut ui = Ui::open_with(renamed_file_repo).await;
    assert!(ui.shows("Find ⌘F"));

    // From the working tree: History comes to the front, hidden or not.
    ui.app.settings.appearance.show_history = false;
    let _ = ui.send(Message::CodeView(diff_view::Role::Local)).await;
    let _ = ui.send(command_f()).await;
    let tab = ui.app.active_tab().expect("tab");
    assert!(tab.history.file_search_is_open());
    assert_eq!(tab.code_view, diff_view::Role::Commit);
    assert!(ui.app.settings.appearance.show_history);

    // Already open: ⌘F keeps what was typed.
    let _ = ui.send(search_query("src/core")).await;
    let _ = ui.send(command_f()).await;
    let history = &ui.app.active_tab().expect("tab").history;
    assert!(history.file_search_is_open());
    assert_eq!(
        history.file_search_state().label_for_hit(0),
        Some("src/core.rs")
    );

    // Esc closes it, ⌘F opens it again.
    let _ = ui.send(esc()).await;
    assert!(
        !ui.app
            .active_tab()
            .expect("tab")
            .history
            .file_search_is_open()
    );
    let _ = ui.send(command_f()).await;
    assert!(
        ui.app
            .active_tab()
            .expect("tab")
            .history
            .file_search_is_open()
    );
}

#[tokio::test]
async fn command_f_in_a_file_history_explains_how_to_search_again() {
    let mut ui = Ui::open_with(renamed_file_repo).await;
    let _ = ui.send(command_f()).await;
    let _ = ui.send(search_query("src/core")).await;
    let _ = ui.send(enter()).await;
    assert!(ui.shows("showing: HEAD · src/core.rs"));

    let _ = ui.send(command_f()).await;
    assert!(
        !ui.app
            .active_tab()
            .expect("tab")
            .history
            .file_search_is_open()
    );
    assert!(ui.flash().contains("close this file history"));
}

#[tokio::test]
async fn history_file_search_closes_when_switching_tabs() {
    let mut ui = Ui::open().await;
    let _ = ui.click("Find ⌘F").await;
    assert!(
        ui.app
            .active_tab()
            .expect("first tab")
            .history
            .file_search_is_open()
    );

    let other = Fixture::new();
    let path = other.repo.path().to_path_buf();
    ui.others.push(other);
    let _ = ui.send(Message::Open(path)).await;
    assert_eq!(ui.app.active, 1);
    assert!(!ui.app.tabs[0].history.file_search_is_open());

    let _ = ui.click("Find ⌘F").await;
    assert!(ui.app.tabs[1].history.file_search_is_open());
    let _ = ui.send(Message::Tabs(tabs::Message::Select(0))).await;
    assert_eq!(ui.app.active, 0);
    assert!(!ui.app.tabs[1].history.file_search_is_open());
}

#[tokio::test]
async fn sync_push_confirms_a_force_rule_before_contacting_the_remote() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["commit", "--allow-empty", "-m", "outgoing"]);
        repo.git(&[
            "config",
            "remote.origin.push",
            "+refs/heads/main:refs/heads/main",
        ]);
    })
    .await;
    let before = ui
        .fixture
        .repo
        .git(&["--git-dir=.git/remote.git", "rev-parse", "main"]);
    let _ = ui.send(Message::Toolbar(toolbar::Message::Push)).await;
    let dialog = ui
        .app
        .dialog
        .as_ref()
        .expect("configured force push is reviewed");
    assert_eq!(dialog.title, "Push may overwrite remote refs");
    assert_eq!(dialog.severity, crate::features::dialog::Severity::Delete);
    assert_eq!(
        ui.fixture
            .repo
            .git(&["--git-dir=.git/remote.git", "rev-parse", "main"]),
        before
    );
}

#[tokio::test]
async fn sync_toolbar_disables_pull_and_push_when_up_to_date() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["push", "origin", "--tags"]);
        repo.git(&["fetch", "origin"]);
    })
    .await;
    let effects = ui.click("[ ⇣ pull ]").await;
    assert!(effects.messages.is_empty(), "{:?}", effects.messages);
    let effects = ui.click("[ ⇡ push ]").await;
    assert!(effects.messages.is_empty(), "{:?}", effects.messages);
}

/// What running a click (or a message) did.
#[derive(Debug, Default)]
pub(super) struct Effects {
    /// Messages handled, the clicked ones first, then task outputs.
    pub messages: Vec<String>,
    pub state_changed: bool,
    /// The window looks different afterwards (checked for clicks only).
    pub screen_changed: bool,
    /// The click selected the same file in the changes list and its view was open.
    change_reselected: bool,
    pub repo_changed: bool,
    pub widget_ops: usize,
    pub clipboard: usize,
    pub window: usize,
    /// A task was still running after [`TASK_TIMEOUT`].
    pub timed_out: bool,
}

impl Effects {
    fn silent(&self) -> bool {
        !self.messages.is_empty()
            && !self.screen_changed
            && !self.repo_changed
            && self.widget_ops == 0
            && self.clipboard == 0
            && self.window == 0
    }
}

/// A clickable spot: a piece of text and where it is.
#[derive(Debug, Clone)]
pub(super) struct Target {
    pub label: String,
    pub at: Point,
}

pub(super) struct Ui {
    pub app: App,
    pub fixture: Fixture,
    /// More repositories open in other tabs.
    pub others: Vec<Fixture>,
    /// Whether [`Self::run`] works out what a click changed (the window,
    /// the app, the repository). Off while the sweep only gets back to a
    /// screen it already reached once: those clicks are not checked, and
    /// measuring them was most of the sweep's time.
    measure: bool,
    _home: tempfile::TempDir,
}

std::thread_local! {
    /// Set while [`reach`] rebuilds a screen: the [`Ui`]s made then start
    /// with [`Ui::measure`] off. (Tests run on one thread each.)
    static REACHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl Ui {
    /// gf with the fixture repository open in a tab, fully loaded.
    pub async fn open() -> Self {
        let mut ui = Self::welcome().await;
        let path = ui.fixture.repo.path().to_path_buf();
        let _ = ui.send(Message::Open(path)).await;
        assert_eq!(ui.app.tabs.len(), 1, "the fixture opens in a tab");
        ui
    }

    /// gf with the fixture open after `setup` changed it (a conflict, a
    /// bisect, ...).
    pub async fn open_with(setup: impl FnOnce(&TestRepo)) -> Self {
        let mut ui = Self::welcome().await;
        setup(&ui.fixture.repo);
        let path = ui.fixture.repo.path().to_path_buf();
        let _ = ui.send(Message::Open(path)).await;
        assert_eq!(ui.app.tabs.len(), 1, "the fixture opens in a tab");
        ui
    }

    /// gf with no tab open; the fixture is in the recent list.
    pub async fn welcome() -> Self {
        let fixture = Fixture::new();
        let home = tempfile::tempdir().expect("home");
        let mut settings = gf_config::Settings::default();
        settings.remember_repo(fixture.repo.path());
        let (mut app, _boot) = App::boot(gf_config::AppPaths::under(home.path()), settings);
        // git runs isolated from the developer's config, as in every test.
        app.runner = app
            .runner
            .clone()
            .with_cleared_env()
            .with_env(fixture.repo.env());
        Self {
            app,
            fixture,
            others: Vec::new(),
            measure: !REACHING.get(),
            _home: home,
        }
    }

    /// Runs a message and every task it starts.
    pub async fn send(&mut self, message: Message) -> Effects {
        self.run(vec![message], false).await
    }

    /// Runs `messages` and their tasks; `look` compares the window before
    /// and after (a click must show something).
    async fn run(&mut self, messages: Vec<Message>, look: bool) -> Effects {
        let mut fx = Effects::default();
        if !self.measure {
            let mut tasks = VecDeque::new();
            for m in messages {
                fx.messages.push(short(&m));
                tasks.push_back(self.handle_native(m, &mut fx));
            }
            self.run_tasks(tasks, &mut fx).await;
            return fx;
        }
        let frame = self._home.path().join("frame.png");
        let _ = std::fs::remove_file(self.frame_file(&frame));
        if look {
            let _ = self.looks_like(&frame);
        }
        let state_before = self.state();
        let repo_before = self.repo_state();
        let mut tasks = VecDeque::new();
        for m in messages {
            fx.messages.push(short(&m));
            tasks.push_back(self.handle_native(m, &mut fx));
        }
        self.run_tasks(tasks, &mut fx).await;
        fx.state_changed = self.state() != state_before;
        fx.screen_changed = look && !self.looks_like(&frame);
        fx.repo_changed = self.repo_state() != repo_before;
        fx
    }

    async fn run_tasks(&mut self, mut tasks: VecDeque<Task<Message>>, fx: &mut Effects) {
        while let Some(task) = tasks.pop_front() {
            let Some(mut stream) = runtime::task::into_stream(task) else {
                continue;
            };
            loop {
                match tokio::time::timeout(TASK_TIMEOUT, stream.next()).await {
                    Ok(Some(Action::Output(m))) => {
                        fx.messages.push(short(&m));
                        tasks.push_back(self.handle_native(m, fx));
                    }
                    Ok(Some(Action::Widget(_))) => fx.widget_ops += 1,
                    Ok(Some(Action::Clipboard(_))) => fx.clipboard += 1,
                    Ok(Some(Action::Window(_) | Action::Exit)) => fx.window += 1,
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => {
                        fx.timed_out = true;
                        break;
                    }
                }
            }
        }
    }

    /// Native windows (the folder chooser) cannot open in a test: they
    /// count as what the click did, and the chooser is cancelled.
    fn handle_native(&mut self, m: Message, fx: &mut Effects) -> Task<Message> {
        match m {
            Message::PickFolder => {
                fx.window += 1;
                Task::none()
            }
            m => self.app.update(m),
        }
    }

    /// Whether the window looks like the frame saved at `path` (the first
    /// call saves it).
    fn looks_like(&self, path: &std::path::Path) -> bool {
        let theme = self.app.theme();
        self.simulator()
            .snapshot(&theme)
            .and_then(|s| s.matches_hash(path))
            .expect("render the window")
    }

    /// Where [`Self::looks_like`] keeps the frame saved at `path`.
    fn frame_file(&self, path: &std::path::Path) -> PathBuf {
        let dir = path.parent().expect("dir");
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.extension().is_some_and(|e| e == "sha256")
                    && p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("frame-"))
            })
            .unwrap_or_else(|| dir.join("frame-none.sha256"))
    }

    /// Everything the app holds, as text (the pointer is left out: every
    /// click moves it).
    fn state(&self) -> String {
        let mut s = format!("{:?}", self.app);
        if let Some(start) = s.find("pointer: Pointer {")
            && let Some(len) = s[start..].find("}, ")
        {
            s.replace_range(start..start + len, "");
        }
        s
    }

    /// The repository as git sees it: refs, index and working tree, stashes.
    fn repo_state(&self) -> String {
        let r = &self.fixture.repo;
        // The git commands run at the same time: every click runs them twice.
        let children = [
            &["for-each-ref", "--format=%(refname) %(objectname)"][..],
            &["status", "--porcelain=v2", "--untracked-files=all"],
            &["stash", "list"],
            &["config", "--local", "--list"],
        ]
        .map(|args| {
            let child = r
                .command()
                .args(args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("run git");
            (args, child)
        });
        // The branch HEAD names (`symbolic-ref`); detached (bisect, rebase):
        // none, not an error.
        let head = std::fs::read_to_string(r.path().join(".git/HEAD")).expect("read HEAD");
        let mut state = vec![
            head.strip_prefix("ref: ")
                .unwrap_or_default()
                .trim_end()
                .to_owned(),
        ];
        for (args, child) in children {
            let out = child.wait_with_output().expect("run git");
            assert!(out.status.success(), "git {args:?}: {out:?}");
            state.push(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        state.join("\n")
    }

    fn simulator(&self) -> iced_test::Simulator<'_, Message> {
        let settings = iced_test::core::Settings {
            default_font: crate::platform::mono_font(),
            ..iced_test::core::Settings::default()
        };
        iced_test::Simulator::with_size(settings, WINDOW, self.app.view())
    }

    /// Every piece of text shown, where it can be clicked.
    pub fn targets(&self) -> Vec<Target> {
        let mut found = Vec::new();
        {
            let found = &mut found;
            let mut sim = self.simulator();
            let _ = sim.find(move |c: Candidate<'_>| -> Option<()> {
                if let Candidate::Text {
                    visible_bounds: Some(b),
                    content,
                    ..
                } = c
                    && !content.trim().is_empty()
                    && b.width >= 1.0
                    && b.height >= 1.0
                {
                    found.push(Target {
                        label: content.to_owned(),
                        at: b.center(),
                    });
                }
                None
            });
        }
        found
    }

    /// The messages a press of `button` at `at` produces (nothing runs).
    pub fn press(&self, at: Point, button: mouse::Button) -> Vec<Message> {
        let mut sim = self.simulator();
        sim.point_at(at);
        let _ = sim.simulate([
            iced::Event::Mouse(mouse::Event::CursorMoved { position: at }),
            iced::Event::Mouse(mouse::Event::ButtonPressed(button)),
            iced::Event::Mouse(mouse::Event::ButtonReleased(button)),
        ]);
        // Hovering is the pointer passing by, not what the click did.
        sim.into_messages()
            .filter(|m| {
                let m = format!("{m:?}");
                !m.contains("(Hover(") && !m.ends_with("(DragEnd)")
            })
            .collect()
    }

    /// Drags the mouse from `from` to `to` (a text selection) and runs
    /// what it does.
    /// Drags in one go, without drawing the window in between (for a
    /// widget that follows the drag itself, like the divider).
    pub async fn drag_through(&mut self, from: Point, to: Point) -> Effects {
        let messages = {
            let mut sim = self.simulator();
            sim.point_at(from);
            let _ = sim.simulate([
                iced::Event::Mouse(mouse::Event::CursorMoved { position: from }),
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            ]);
            sim.point_at(to);
            let _ = sim.simulate([
                iced::Event::Mouse(mouse::Event::CursorMoved { position: to }),
                iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ]);
            sim.into_messages().collect()
        };
        self.run(messages, true).await
    }

    pub async fn drag(&mut self, from: Point, to: Point) -> Effects {
        // The window is drawn again between the steps, as it would be.
        let step = |ui: &Self, at: Point, event: mouse::Event| -> Vec<Message> {
            let mut sim = ui.simulator();
            sim.point_at(at);
            let _ = sim.simulate([
                iced::Event::Mouse(mouse::Event::CursorMoved { position: at }),
                iced::Event::Mouse(event),
            ]);
            sim.into_messages().collect()
        };
        let pressed = step(self, from, mouse::Event::ButtonPressed(mouse::Button::Left));
        let mut fx = self.run(pressed, true).await;
        let moved = step(self, to, mouse::Event::CursorMoved { position: to });
        let released = step(self, to, mouse::Event::ButtonReleased(mouse::Button::Left));
        let end = self
            .run(moved.into_iter().chain(released).collect(), true)
            .await;
        fx.messages.extend(end.messages);
        fx.screen_changed |= end.screen_changed;
        fx
    }

    /// Clicks the first text equal to `label` (or starting with it) and
    /// runs what it does.
    pub async fn click(&mut self, label: &str) -> Effects {
        self.click_with(label, mouse::Button::Left).await
    }

    pub async fn right_click(&mut self, label: &str) -> Effects {
        self.click_with(label, mouse::Button::Right).await
    }

    async fn click_with(&mut self, label: &str, button: mouse::Button) -> Effects {
        self.settle();
        // Tests name shortcuts with mac glyphs, as the code writes them.
        let label = &*crate::platform::keys::local(label.into());
        let targets = self.targets();
        let target = targets
            .iter()
            .find(|t| t.label == label)
            .or_else(|| targets.iter().find(|t| t.label.trim().starts_with(label)))
            .or_else(|| targets.iter().find(|t| t.label.contains(label)))
            .unwrap_or_else(|| {
                panic!(
                    "no {label:?} on screen; shown: {:?}",
                    targets.iter().map(|t| &t.label).collect::<Vec<_>>()
                )
            })
            .clone();
        self.click_at(&target, button).await
    }

    pub async fn click_at(&mut self, target: &Target, button: mouse::Button) -> Effects {
        let messages = self.press(target.at, button);
        let change_reselected = self.reselects_open_change(&messages);
        let mut effects = self.run(messages, true).await;
        effects.change_reselected = change_reselected;
        effects
    }

    fn reselects_open_change(&self, messages: &[Message]) -> bool {
        let [Message::Changes(changes::Message::Select(path, group))] = messages else {
            return false;
        };
        self.app.active_tab().is_some_and(|tab| {
            tab.code_view == diff_view::Role::Local
                && tab.changes.is_selected(path, *group)
                && match (&tab.conflict, group) {
                    (Some(conflict), changes::Group::Conflicts) => conflict.path == *path,
                    (None, changes::Group::Conflicts) | (Some(_), _) => false,
                    (None, group) => {
                        tab.local_diff.source() == Some(&changes::diff_source(path.clone(), *group))
                    }
                }
        })
    }

    /// Clears the flash: one already shown would hide one the next click
    /// shows again. Done before looking for targets, as the status line
    /// is laid out differently with a flash.
    pub fn settle(&mut self) {
        self.app.flash = None;
    }

    pub fn shows(&self, label: &str) -> bool {
        let label = &*crate::platform::keys::local(label.into());
        self.targets().iter().any(|t| t.label.contains(label))
    }

    pub fn flash(&self) -> String {
        self.app
            .flash
            .as_ref()
            .map(|f| f.text.clone())
            .unwrap_or_default()
    }
}

/// A message, shortened for reports.
fn short(m: &Message) -> String {
    let s = format!("{m:?}");
    if s.len() > 160 {
        let cut = (0..=160)
            .rev()
            .find(|i| s.is_char_boundary(*i))
            .unwrap_or(0);
        format!("{}…", &s[..cut])
    } else {
        s
    }
}

/// What kind of click a message is, to try a few of many alike rows (a
/// commit, a file, a branch) but every distinct button.
fn kind(m: &str) -> String {
    let row_like = ["Oid(", "Ref(", "\"", "Point {"];
    if !row_like.iter().any(|p| m.contains(p)) {
        return m.to_owned();
    }
    let mut depth = 0;
    let mut end = m.len();
    for (i, c) in m.char_indices() {
        if c == '(' || c == '{' || c == ' ' {
            depth += 1;
            if depth == 2 {
                end = i;
                break;
            }
        }
    }
    m[..end].to_owned()
}

/// Picking what is already picked (the shown tab, the mode or pane in
/// use, the chosen option) rightly changes nothing.
fn reselect(messages: &[String]) -> bool {
    let picks = [
        "Tabs(Select(",
        "CodeView(",
        "FocusPane(",
        "Dialog(Choose(",
        "History(Select(",
        "Sidebar(Select(",
        "CommitDiff(SelectFile(",
        "LocalDiff(SelectFile(",
        "Blame(Select(",
        "CodeDialog(Pressed(",
    ];
    messages.len() == 1 && picks.iter().any(|p| messages[0].starts_with(p))
}

/// One click that did nothing.
#[derive(Debug)]
pub(super) struct Silent {
    pub screen: String,
    pub label: String,
    pub button: &'static str,
    pub messages: Vec<String>,
}

/// How to get to a screen from the base one: the clicks, by label.
type Path = Vec<(String, mouse::Button)>;

struct Screen {
    name: String,
    path: Path,
    /// Labels the screen it was reached from already had: only the new
    /// ones are tried (the rest were, there).
    known: Option<std::collections::HashSet<String>>,
}

/// How many screens deep from the base screen clicks are followed: one
/// by default (the dialogs and menus the base screen opens),
/// `GF_SWEEP_DEPTH=2` for what those open too (about ten minutes).
fn depth() -> usize {
    std::env::var("GF_SWEEP_DEPTH")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(1)
}

/// Sweeps the screen `build` gives and the screens its clicks lead to
/// (dialogs, menus, a selected commit's files, ...). Returns the clicks
/// that did nothing.
pub(super) async fn sweep<F, Fut>(name: &str, build: F) -> Vec<Silent>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Ui>,
{
    let mut silent = Vec::new();
    let mut screens = VecDeque::from([Screen {
        name: name.to_owned(),
        path: Vec::new(),
        known: None,
    }]);
    let mut seen_screens = std::collections::HashSet::new();
    let mut tried = std::collections::HashSet::new();
    let mut first = true;
    while let Some(screen) = screens.pop_front() {
        let Some(base) = reach(&build, &screen.path, std::mem::take(&mut first)).await else {
            continue;
        };
        let targets = base.targets();
        let labels: std::collections::HashSet<String> =
            targets.iter().map(|t| t.label.clone()).collect();
        // Which spots react at all, a couple per kind of message.
        let mut by_kind: BTreeMap<(String, &'static str), usize> = BTreeMap::new();
        let mut plan = Vec::new();
        for target in &targets {
            if screen
                .known
                .as_ref()
                .is_some_and(|k| k.contains(&target.label))
            {
                continue;
            }
            for (button, button_name) in [
                (mouse::Button::Left, "click"),
                (mouse::Button::Right, "right-click"),
            ] {
                let messages = base.press(target.at, button);
                if messages.is_empty() {
                    continue;
                }
                let k: Vec<String> = messages.iter().map(|m| kind(&short(m))).collect();
                let key = (k.join(" "), button_name);
                let n = by_kind.entry(key.clone()).or_default();
                *n += 1;
                // One row of a kind (a commit, a file), every button; once
                // overall when it does not depend on the screen.
                if *n > 1 || (screen.known.is_some() && tried.contains(&key)) {
                    continue;
                }
                plan.push((target.clone(), button, button_name, key));
            }
        }
        let modal_before = modal(&base.app);
        drop(base);
        for (target, button, button_name, key) in plan {
            let _ = tried.insert(key.clone());
            let Some(mut ui) = reach(&build, &screen.path, false).await else {
                continue;
            };
            let fx = ui.click_at(&target, button).await;
            let quiet = fx.silent() && !fx.change_reselected && !reselect(&fx.messages);
            println!(
                "{} › {button_name} {:?} → {}{}",
                screen.name,
                target.label.trim(),
                fx.messages.join(" · "),
                if quiet { "  [SILENT]" } else { "" }
            );
            if quiet {
                silent.push(Silent {
                    screen: screen.name.clone(),
                    label: target.label.clone(),
                    button: button_name,
                    messages: fx.messages.clone(),
                });
            }
            // Another tab is another screen, swept by its own test.
            let elsewhere = [
                "Welcome(OpenRecent",
                "Tabs(Select",
                "Tabs(Close",
                "TabList(Pick",
                "TabList(CloseTab",
                "Picker(Pick",
            ];
            if screen.path.len() >= depth()
                || !(fx.state_changed || fx.repo_changed)
                || elsewhere.iter().any(|e| fx.messages[0].starts_with(e))
            {
                continue;
            }
            // Follow clicks that show new buttons: a dialog or menu, or
            // what a selection brings up.
            ui.settle();
            let opened = modal(&ui.app);
            let new_labels = ui.targets().iter().any(|t| !labels.contains(&t.label));
            if !(new_labels || opened != modal_before) {
                continue;
            }
            let id = format!("{opened:?} {}", key.0);
            if seen_screens.insert(id) {
                let mut path = screen.path.clone();
                path.push((target.label.clone(), button));
                screens.push_back(Screen {
                    name: format!("{} › {button_name} {:?}", screen.name, target.label.trim()),
                    path,
                    known: (opened == modal_before).then(|| labels.clone()),
                });
            }
        }
    }
    silent
}

/// The base screen, then the clicks along `path`; `first` for the sweep's
/// first build.
async fn reach<F, Fut>(build: &F, path: &Path, first: bool) -> Option<Ui>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Ui>,
{
    // Only the first build is measured (it checks what `build` asserts
    // about its clicks); later ones only get back to the same screen.
    REACHING.set(!first);
    let ui = async {
        let mut ui = build().await;
        ui.settle();
        for (label, button) in path {
            let targets = ui.targets();
            let target = targets.iter().find(|t| &t.label == label)?.clone();
            let _ = ui.click_at(&target, *button).await;
            ui.settle();
        }
        Some(ui)
    }
    .await;
    REACHING.set(false);
    let mut ui = ui?;
    // The click the sweep makes next is checked.
    ui.measure = true;
    Some(ui)
}

/// Which layer above the main window is open, if any.
fn modal(app: &App) -> Option<&'static str> {
    if app.dialog.is_some() {
        Some("dialog")
    } else if app.menu.is_some() {
        Some("menu")
    } else if app.code_dialog.is_some() {
        Some("code dialog")
    } else if app.settings_open {
        Some("settings")
    } else if app.picker.is_some() {
        Some("picker")
    } else if app.tab_list.is_some() {
        Some("tab list")
    } else if app.undo_list.is_some() {
        Some("undo list")
    } else if app.rebase_editor().is_some() {
        Some("rebase editor")
    } else {
        None
    }
}

pub(super) fn report(silent: &[Silent]) -> String {
    silent
        .iter()
        .map(|s| {
            format!(
                "{} › {} {:?}\n    {}",
                s.screen,
                s.button,
                s.label.trim(),
                s.messages.join("\n    ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_welcome() {
    let silent = sweep("welcome", Ui::welcome).await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_history() {
    let silent = sweep("history", Ui::open).await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

#[tokio::test]
async fn squash_into_previous_explains_why_the_root_commit_cannot_be_used() {
    let mut ui = Ui::open().await;
    let _ = ui.right_click("first").await;
    let fx = ui.click("squash into previous").await;
    assert!(!fx.repo_changed);
    assert!(ui.shows("Cannot squash"));
    assert!(ui.shows("first commit has no previous commit"));
    assert!(ui.shows("select it together with the commits after it"));
}

#[tokio::test]
async fn squash_on_another_branch_explains_which_branch_to_use() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["reset", "--hard"]);
        repo.git(&["clean", "-fd"]);
        repo.git(&["switch", "feature"]);
        repo.write("feature-only.txt", "branch-only content");
        repo.commit_all("feature only commit");
        repo.git(&["switch", "main"]);
    })
    .await;
    let _ = ui.right_click("feature only commit").await;
    let fx = ui.click("squash into previous").await;
    assert!(!fx.repo_changed);
    assert!(ui.shows("Cannot squash"));
    assert!(ui.shows("not on the current branch"));
    assert!(ui.shows("check out the branch"));
}

#[tokio::test]
async fn squash_in_editor_explains_how_to_include_the_root_commit() {
    let mut ui = Ui::open().await;
    let root = ui
        .fixture
        .repo
        .git(&["rev-parse", "HEAD~2"])
        .trim()
        .parse()
        .expect("root id");
    let _ = ui.click("second").await;
    let _ = ui
        .send(Message::History(history::Message::SelectRange(root)))
        .await;
    let _ = ui.right_click("second").await;
    let fx = ui.click("squash in rebase editor").await;
    assert!(!fx.repo_changed);
    assert!(ui.shows("Cannot squash"));
    assert!(ui.shows("needs a commit before the selected range"));
    assert!(ui.shows("use “squash into one” in the selection menu"));
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_working_tree() {
    let silent = sweep("working tree", || async {
        let mut ui = Ui::open().await;
        let _ = ui.send(Message::CodeView(diff_view::Role::Local)).await;
        ui
    })
    .await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

/// Main and `feature` both change the first line of `a.txt`; the work
/// tree is clean.
fn diverged(repo: &TestRepo) {
    repo.git(&["reset", "-q", "--hard"]);
    repo.git(&["clean", "-qfd"]);
    repo.git(&["switch", "-q", "feature"]);
    repo.write("a.txt", "feature\n");
    repo.commit_all("feature edit");
    repo.git(&["switch", "-q", "main"]);
    repo.write("a.txt", "main\n");
    repo.commit_all("main edit");
}

async fn working_tree(ui: Ui) -> Ui {
    let mut ui = ui;
    let _ = ui.send(Message::CodeView(diff_view::Role::Local)).await;
    ui
}

fn file_status_x(ui: &Ui, filename: &str, code: &str) -> f32 {
    let targets = ui.targets();
    let file = targets
        .iter()
        .find(|t| t.label == filename)
        .expect("file row");
    let status = targets
        .iter()
        .find(|t| t.label == code && (t.at.y - file.at.y).abs() < 2.0)
        .expect("status column");
    assert!(status.at.x < file.at.x);
    status.at.x
}

#[tokio::test]
async fn working_tree_defaults_to_list_and_tree_folds_all_files_in_a_folder() {
    let mut ui = working_tree(
        Ui::open_with(|r| {
            r.write("src/staged/deep/staged.txt", "staged\n");
            r.git(&["add", "src/staged/deep/staged.txt"]);
            r.write("src/lib.rs", "unstaged\n");
        })
        .await,
    )
    .await;
    assert!(!ui.app.settings.appearance.working_tree_dirs);
    assert!(!ui.shows("STAGED ("));
    let _ = ui.click(file_tree::MODE_GLYPH).await;
    assert!(ui.shows("staged.txt"));
    assert!(ui.shows("lib.rs"));
    assert!((file_status_x(&ui, "b.txt", "A") - file_status_x(&ui, "staged.txt", "A")).abs() < 1.0);
    let _ = ui.click("src").await;
    assert!(!ui.shows("staged.txt"));
    assert!(!ui.shows("lib.rs"));
    assert!(ui.shows("b.txt"));
    let moved = ui
        .app
        .active_tab()
        .expect("repository")
        .changes
        .nav(
            crate::nav::Nav::plain(crate::nav::Key::Enter),
            ui.app
                .active_tab()
                .expect("repository")
                .snapshot
                .as_deref()
                .expect("status"),
            &ui.app.tokens,
            true,
        )
        .and_then(|m| m.message)
        .expect("Enter expands the focused folder");
    let _ = ui.send(Message::Changes(moved)).await;
    assert!(ui.shows("staged.txt"));
    let _ = ui.click(file_tree::MODE_GLYPH).await;
    let saved = Settings::load(&ui.app.paths.settings_file()).expect("saved settings");
    assert!(!saved.appearance.working_tree_dirs);
    assert!(saved.appearance.history_file_dirs);
}

/// One working tree key press in tree mode, applied to the app.
async fn tree_key(ui: &mut Ui, key: crate::nav::Key) -> Option<changes::Message> {
    let tab = ui.app.active_tab().expect("repository");
    let snap = tab.snapshot.as_deref().expect("status");
    let message = tab
        .changes
        .nav(crate::nav::Nav::plain(key), snap, &ui.app.tokens, true)
        .and_then(|m| m.message);
    if let Some(message) = message.clone() {
        let _ = ui.send(Message::Changes(message)).await;
    }
    message
}

#[tokio::test]
async fn working_tree_arrows_skip_open_folders_and_minus_folds_the_current_one() {
    use crate::nav::Key;
    use changes::Message as M;
    let mut ui = working_tree(
        Ui::open_with(|r| {
            r.write("src/staged/deep/staged.txt", "staged\n");
            r.write("src/lib.rs", "unstaged\n");
        })
        .await,
    )
    .await;
    let _ = ui.click(file_tree::MODE_GLYPH).await;
    let selected =
        |m: &Option<M>, name: &str| matches!(m, Some(M::Select(path, _)) if path.ends_with(name));
    // Rows: src/ · staged/deep/ · staged.txt · lib.rs · then the root files.
    let m = tree_key(&mut ui, Key::Home).await;
    assert!(selected(&m, "staged.txt"), "Home skips open folders: {m:?}");
    let m = tree_key(&mut ui, Key::Down).await;
    assert!(selected(&m, "lib.rs"), "{m:?}");
    let m = tree_key(&mut ui, Key::Up).await;
    assert!(selected(&m, "staged.txt"), "{m:?}");
    assert!(
        tree_key(&mut ui, Key::Up).await.is_none(),
        "nothing above but folders"
    );

    let m = tree_key(&mut ui, Key::Char('-')).await;
    assert!(
        matches!(&m, Some(M::ToggleFolder(p)) if p == std::path::Path::new("src/staged")),
        "{m:?}"
    );
    // The selected file's name stays in the diff title, so check the rows by moving.
    let m = tree_key(&mut ui, Key::Down).await;
    assert!(selected(&m, "lib.rs"), "staged.txt is folded away: {m:?}");
    let m = tree_key(&mut ui, Key::Up).await;
    assert!(
        matches!(&m, Some(M::FocusFolder(p)) if p == std::path::Path::new("src/staged")),
        "a folded folder is a stop: {m:?}"
    );
    let _ = tree_key(&mut ui, Key::Enter).await;
    let m = tree_key(&mut ui, Key::Down).await;
    assert!(selected(&m, "staged.txt"), "Enter unfolded it: {m:?}");

    let _ = tree_key(&mut ui, Key::Down).await;
    let m = tree_key(&mut ui, Key::Char('-')).await;
    assert!(
        matches!(&m, Some(M::ToggleFolder(p)) if p == std::path::Path::new("src")),
        "lib.rs folds src: {m:?}"
    );
    assert!(
        tree_key(&mut ui, Key::Home).await.is_none(),
        "already on src/"
    );
    let m = tree_key(&mut ui, Key::Down).await;
    assert!(
        matches!(&m, Some(M::Select(p, _)) if !p.starts_with("src")),
        "src/ is folded: {m:?}"
    );
}

#[tokio::test]
async fn working_tree_shift_click_marks_files_for_one_menu_and_esc_clears_them() {
    let mut ui = working_tree(
        Ui::open_with(|r| {
            for name in ["x1.txt", "x2.txt", "x3.txt"] {
                r.write(name, "new\n");
            }
        })
        .await,
    )
    .await;
    let _ = ui.click("x1.txt").await;
    let _ = ui
        .send(Message::Modifiers(keyboard::Modifiers::SHIFT))
        .await;
    let _ = ui.click("x3.txt").await;
    let _ = ui
        .send(Message::Modifiers(keyboard::Modifiers::empty()))
        .await;
    let _ = ui.right_click("x2.txt").await;
    assert!(ui.shows("3 files selected"));
    assert!(ui.shows("discard changes… (3)"));
    let _ = ui.send(Message::Key(Shortcut::Escape)).await;
    assert!(!ui.shows("3 files selected"), "Esc closes the menu first");

    let _ = ui.send(Message::Key(Shortcut::Escape)).await;
    let _ = ui.right_click("x2.txt").await;
    assert!(!ui.shows("files selected"), "Esc cleared the marks");
    let _ = ui.send(Message::Key(Shortcut::Escape)).await;

    // ⌘-click adds to the current file.
    let _ = ui.click("x1.txt").await;
    let _ = ui
        .send(Message::Modifiers(keyboard::Modifiers::COMMAND))
        .await;
    let _ = ui.click("x3.txt").await;
    let _ = ui
        .send(Message::Modifiers(keyboard::Modifiers::empty()))
        .await;
    let _ = ui.right_click("x1.txt").await;
    assert!(ui.shows("2 files selected"));
    let _ = ui.send(Message::Key(Shortcut::Escape)).await;

    // ⌘A in the file list marks every changed file.
    let _ = ui.send(Message::Key(Shortcut::SelectAll)).await;
    let snap = ui
        .app
        .active_tab()
        .and_then(|tab| tab.snapshot.clone())
        .expect("status");
    let all = snap.change_files.paths(&snap.status.entries).len();
    assert!(all > 3, "the fixture's own changes too");
    let _ = ui.right_click("x2.txt").await;
    assert!(ui.shows(&format!("{all} files selected")));
}

#[tokio::test]
async fn history_tree_places_renames_under_the_destination_and_opens_the_diff() {
    let mut ui = Ui::open_with(|r| {
        r.write("moved/deep/target/keep.txt", "directory\n");
        r.git(&["mv", "src/lib.rs", "moved/deep/target/lib_new.rs"]);
        r.git(&["add", "moved/deep/target/keep.txt"]);
        r.git(&["commit", "-qm", "move source file"]);
    })
    .await;
    let _ = ui.click("move source file").await;
    assert!(ui.app.settings.appearance.history_file_dirs);
    let labels: Vec<_> = ui
        .targets()
        .into_iter()
        .map(|target| target.label)
        .collect();
    assert!(labels.iter().any(|label| label == "moved/deep/target/"));
    assert!(
        !labels
            .iter()
            .any(|label| label == "moved/" || label == "src/")
    );
    assert!(ui.shows("lib.rs → lib_new.rs"));
    assert!(
        (file_status_x(&ui, "b.txt", "A") - file_status_x(&ui, "lib.rs → lib_new.rs", "R")).abs()
            < 1.0
    );
    let _ = ui.click("lib.rs → lib_new.rs").await;
    assert_eq!(
        ui.app
            .active_tab()
            .expect("repository")
            .commit_diff
            .current_path()
            .as_deref(),
        Some(std::path::Path::new("moved/deep/target/lib_new.rs"))
    );
    let _ = ui.click("moved/deep/target/").await;
    assert!(!ui.shows("lib.rs → lib_new.rs"));
    let _ = ui.click(file_tree::MODE_GLYPH).await;
    assert!(ui.shows("lib_new.rs"));
}

fn stage_checkbox(ui: &Ui, name: &str, checked: bool) -> Target {
    let file = ui
        .targets()
        .into_iter()
        .find(|target| target.label == name)
        .expect("file row");
    let target = (0..140_u16).step_by(3).find_map(|x| {
        let at = Point::new(file.at.x - f32::from(x), file.at.y);
        match ui.press(at, mouse::Button::Left).as_slice() {
            [Message::Changes(changes::Message::Stage(path))]
                if !checked && path.ends_with(name) =>
            {
                Some(Target {
                    label: name.into(),
                    at,
                })
            }
            [Message::Changes(changes::Message::Unstage(path))]
                if checked && path.ends_with(name) =>
            {
                Some(Target {
                    label: name.into(),
                    at,
                })
            }
            _ => None,
        }
    });
    target.expect("staging checkbox")
}

#[tokio::test]
async fn staging_checkboxes_commit_only_the_checked_changes() {
    let mut ui = working_tree(Ui::open().await).await;
    for (name, checked) in [("b.txt", true), ("a.txt", false)] {
        ui.settle();
        let effects = ui
            .click_at(&stage_checkbox(&ui, name, checked), mouse::Button::Left)
            .await;
        assert!(effects.repo_changed, "{name}");
    }
    let staged = ui.fixture.repo.git(&["show", ":a.txt"]);
    ui.fixture.repo.write("a.txt", "a new unstaged edit\n");
    let _ = ui.send(Message::Toolbar(toolbar::Message::Refresh)).await;
    assert_eq!(
        ui.targets().iter().filter(|t| t.label == "a.txt").count(),
        1
    );
    let _ = stage_checkbox(&ui, "a.txt", false);
    let _ = ui.right_click("a.txt").await;
    assert!(ui.shows("staged diff"));
    let _ = ui.click("staged diff").await;
    assert_eq!(
        ui.app.active_tab().expect("tab").local_diff.source(),
        Some(&diff_view::DiffSource::Staged("a.txt".into()))
    );
    let _ = ui
        .send(Message::Commit(commit_box::Message::Summary(
            "commit the checked file".into(),
        )))
        .await;
    let _ = ui
        .send(Message::Commit(commit_box::Message::Submit {
            amend: false,
        }))
        .await;
    assert_eq!(
        ui.fixture
            .repo
            .git(&["show", "--format=", "--name-only", "HEAD"])
            .trim(),
        "a.txt"
    );
    assert!(
        ui.fixture
            .repo
            .git(&["status", "--short"])
            .contains("?? b.txt")
    );
    assert_eq!(ui.fixture.repo.git(&["show", "HEAD:a.txt"]), staged);
    assert!(
        ui.fixture
            .repo
            .git(&["diff", "--", "a.txt"])
            .contains("a new unstaged edit")
    );
}

#[tokio::test]
async fn staging_checkboxes_update_immediately_and_serialize_rapid_clicks() {
    let mut ui = working_tree(Ui::open().await).await;
    let a = stage_checkbox(&ui, "a.txt", false);
    let first = ui.app.update(ui.press(a.at, mouse::Button::Left).remove(0));
    // Neither the Git task nor a status refresh has run yet.
    assert_eq!(
        ui.fixture
            .repo
            .git(&["diff", "--cached", "--name-only"])
            .trim(),
        "b.txt"
    );
    let a = stage_checkbox(&ui, "a.txt", true);
    let b = stage_checkbox(&ui, "b.txt", true);
    let c = stage_checkbox(&ui, "c.txt", false);
    let mut tasks = VecDeque::from([first]);
    for target in [b, c, a] {
        tasks.push_back(
            ui.app
                .update(ui.press(target.at, mouse::Button::Left).remove(0)),
        );
    }
    let _ = stage_checkbox(&ui, "a.txt", false);
    let _ = stage_checkbox(&ui, "c.txt", true);
    ui.run_tasks(tasks, &mut Effects::default()).await;
    assert_eq!(
        ui.fixture
            .repo
            .git(&["diff", "--cached", "--name-only"])
            .trim(),
        "c.txt"
    );
    assert!(ui.app.stage_files.is_empty());
    assert!(ui.app.active_tab().expect("tab").stage_update.is_none());
}

#[tokio::test]
async fn commit_shortcut_waits_until_staging_finishes() {
    let mut ui = working_tree(Ui::open().await).await;
    let before = ui.fixture.repo.git(&["rev-parse", "HEAD"]);
    let _ = ui
        .send(Message::Commit(commit_box::Message::Summary(
            "commit after unstaging".into(),
        )))
        .await;
    let _pending = ui
        .app
        .update(Message::Changes(changes::Message::Unstage("b.txt".into())));
    let _ = ui
        .send(Message::Commit(commit_box::Message::Submit {
            amend: false,
        }))
        .await;
    assert!(ui.flash().contains("wait for staged changes"));
    assert_eq!(ui.fixture.repo.git(&["rev-parse", "HEAD"]), before);
}

#[tokio::test]
async fn sweep_recognizes_reselecting_a_file_after_copying_its_path() {
    let mut ui = working_tree(Ui::open().await).await;
    let _ = ui.right_click("b.txt").await;
    assert_eq!(ui.click("copy path").await.clipboard, 1);

    let selected = ui.click("b.txt").await;
    assert!(selected.change_reselected);
    assert!(
        selected
            .messages
            .iter()
            .any(|m| m.starts_with("DiffFinished("))
    );

    let other = ui.click("a.txt").await;
    assert!(!other.change_reselected);
    assert!(other.screen_changed);
}

#[tokio::test]
async fn reselecting_an_open_conflict_keeps_its_choices() {
    let mut ui = working_tree(
        Ui::open_with(|r| {
            diverged(r);
            let _ = r.git_output(&["merge", "feature"]);
        })
        .await,
    )
    .await;
    let _ = ui.right_click("a.txt").await;
    let opened = ui.click("open resolver").await;
    assert!(!opened.change_reselected);
    assert!(ui.shows("RESOLVE CONFLICT"));

    let _ = ui
        .send(Message::Conflict(
            crate::features::conflict_view::Message::Choose(0, gf_git::conflict::Choice::Ours),
        ))
        .await;
    let selected = ui.click("a.txt").await;
    assert!(selected.change_reselected);
    assert!(ui.shows("all conflicts chosen"));
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_merge_conflict() {
    let silent = sweep("merge conflict", || async {
        working_tree(
            Ui::open_with(|r| {
                diverged(r);
                let _ = r.git_output(&["merge", "feature"]);
            })
            .await,
        )
        .await
    })
    .await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_rebase_stopped() {
    let silent = sweep("rebase stopped", || async {
        working_tree(
            Ui::open_with(|r| {
                diverged(r);
                let _ = r.git_output(&["rebase", "feature"]);
            })
            .await,
        )
        .await
    })
    .await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_bisect() {
    let silent = sweep("bisect", || async {
        Ui::open_with(|r| {
            r.git(&["reset", "-q", "--hard"]);
            r.git(&["clean", "-qfd"]);
            r.git(&["bisect", "start", "HEAD", "HEAD~2"]);
        })
        .await
    })
    .await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_two_tabs_and_undo() {
    let silent = sweep("two tabs, undo", || async {
        let mut ui = Ui::open().await;
        // A second tab, then back to the first.
        let other = Fixture::new();
        let path = other.repo.path().to_path_buf();
        ui.others.push(other);
        let _ = ui.send(Message::Open(path)).await;
        let _ = ui.send(Message::Tabs(tabs::Message::Select(0))).await;
        let _ = ui
            .send(Message::Changes(changes::Message::Stage("a.txt".into())))
            .await;
        assert!(ui.app.tabs.len() == 2, "two tabs");
        ui
    })
    .await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

/// The working tree with `a.txt`'s unstaged changes shown.
async fn a_txt_shown() -> Ui {
    let mut ui = working_tree(Ui::open().await).await;
    let _ = ui
        .send(Message::Changes(changes::Message::Select(
            "a.txt".into(),
            changes::Group::Unstaged,
        )))
        .await;
    ui
}

fn code_close_target(ui: &Ui) -> Target {
    ui.targets()
        .into_iter()
        .find(|target| {
            target.label == "✕"
                && ui
                    .press(target.at, mouse::Button::Left)
                    .iter()
                    .any(|message| {
                        matches!(message, Message::CodeDialog(code_view::Message::Close))
                    })
        })
        .expect("code dialog close icon")
}

#[tokio::test]
async fn local_view_keeps_keyboard_copy_and_working_tree_menu_opens_plain_file() {
    let mut ui = a_txt_shown().await;
    assert!(!ui.shows("[ View file ]"));
    let opened = ui.click("[ View ]").await;
    assert!(!opened.repo_changed);
    assert!(ui.app.code_dialog.is_some());
    assert!(ui.shows("FILE CHANGES"));
    let mut first_line = ui
        .targets()
        .into_iter()
        .find(|t| t.label == "  1")
        .expect("first line");
    first_line.at.x += ui.app.tokens.cols(5);
    let _ = ui.click_at(&first_line, mouse::Button::Left).await;
    assert!(
        ui.app.code_dialog.is_some(),
        "clicks stay inside the dialog"
    );
    let selected = ui
        .send(Message::KeyInput(focus::KeyInput {
            shortcut: Some(Shortcut::SelectAll),
            menu: None,
            nav: None,
        }))
        .await;
    assert!(selected.state_changed);
    assert!(!ui.shows("[ Copy ⌘C ]"));
    let copied = ui
        .send(Message::KeyInput(focus::KeyInput {
            shortcut: Some(Shortcut::Copy),
            menu: None,
            nav: None,
        }))
        .await;
    assert_eq!(copied.clipboard, 1);
    assert!(!copied.repo_changed);
    let _ = ui
        .send(Message::KeyInput(focus::KeyInput {
            shortcut: Some(Shortcut::Escape),
            menu: None,
            nav: None,
        }))
        .await;
    assert!(ui.app.code_dialog.is_none());

    let _ = ui.right_click("a.txt").await;
    assert!(ui.app.menu.is_some());
    let opened = ui.click("view file").await;
    assert!(!opened.repo_changed);
    assert!(ui.shows("FILE CONTENTS"));
    assert!(!ui.shows("[ Side by side ]"));
    assert!(!ui.shows("FILE CHANGES"));
    let viewer = ui.app.code_dialog.as_mut().expect("file dialog");
    let _ = viewer.update(code_view::Message::SelectAll);
    assert!(matches!(
        viewer.update(code_view::Message::Copy).output,
        Some(code_view::Output::Copy(text)) if text == "one\ntwo\nchanged\nfour\n"
    ));
}

#[tokio::test]
async fn history_files_menu_previews_the_clicked_files_commit_version() {
    let mut ui = Ui::open().await;
    let revision = ui.fixture.repo.git(&["rev-parse", "HEAD~2"]);
    let _ = ui.click("first").await;
    let clicked = ui.right_click("lib.rs").await;
    assert!(!clicked.repo_changed);
    assert!(ui.shows("file history"));
    assert!(ui.shows("blame"));
    assert!(ui.shows(&format!("reveal in {}", crate::platform::FILE_MANAGER)));
    let copied = ui.click("copy path").await;
    assert_eq!(copied.clipboard, 1);
    assert_eq!(ui.flash(), "copied src/lib.rs");

    let _ = ui.right_click("lib.rs").await;
    let opened = ui.click("view file").await;
    assert!(!opened.repo_changed);
    assert!(ui.app.code_dialog.is_some());
    assert!(ui.shows("FILE CONTENTS"));
    assert!(ui.shows("src/lib.rs"));
    assert!(ui.shows(revision.trim()));
    assert!(ui.targets().iter().any(|t| t.label == "  1"));
    assert!(!ui.shows("println!"));
    assert!(!ui.shows("[ Side by side ]"));

    let close = code_close_target(&ui);
    let _ = ui.click_at(&close, mouse::Button::Left).await;
    assert!(ui.app.code_dialog.is_none());
    let _ = ui.right_click("lib.rs").await;
    let _ = ui.click("file history").await;
    assert!(ui.shows("first"));
    assert!(!ui.shows("third"));
}

#[tokio::test]
async fn commit_view_compares_the_selected_commit_with_its_parent() {
    let mut ui = Ui::open().await;
    let _ = ui.click("second").await;
    let opened = ui.click("[ View ]").await;
    assert!(!opened.repo_changed);
    assert!(ui.shows("FILE CHANGES"));
    let targets = ui.targets();
    let added = targets
        .iter()
        .find(|t| t.label == "  4")
        .expect("the added line number");
    assert!(
        targets.iter().any(|t| {
            t.label == "+" && t.at.x > added.at.x && (t.at.y - added.at.y).abs() < 1.0
        })
    );
    let split = ui.click("[ Side by side ]").await;
    assert!(split.screen_changed);
    assert!(ui.shows("Parent"));
    assert!(ui.shows("Commit"));

    let viewer = ui.app.code_dialog.as_mut().expect("file dialog");
    let _ = viewer.update(code_view::Message::SelectAll);
    assert!(matches!(
        viewer.update(code_view::Message::Copy).output,
        Some(code_view::Output::Copy(text)) if text == "one\ntwo\nthree\nfour\n"
    ));
}

#[tokio::test]
async fn history_files_preview_uses_the_parent_for_a_deleted_file() {
    let mut ui = Ui::open_with(|repo| {
        repo.write("gone.txt", "content before deletion\n");
        repo.commit_all("add gone");
        repo.git(&["rm", "gone.txt"]);
        repo.git(&["commit", "-qm", "remove gone"]);
    })
    .await;
    let _ = ui.click("remove gone").await;
    let _ = ui.right_click("gone.txt").await;
    let _ = ui.click("view file").await;
    assert!(ui.app.code_dialog.is_some());
    assert!(ui.shows("FILE CONTENTS"));
    let line_number = ui.targets().into_iter().find(|t| t.label == "  1");
    assert!(line_number.is_some(), "the parent blob was loaded");
    let close = code_close_target(&ui);
    let _ = ui.click_at(&close, mouse::Button::Left).await;
    let _ = ui.click("[ View ]").await;
    assert!(ui.shows("FILE CHANGES"));
    assert!(ui.shows("−"));
    assert!(ui.shows("[ Side by side ]"));
}

#[tokio::test]
async fn oversized_file_shows_preview_limit_and_cannot_be_copied() {
    let mut ui = working_tree(
        Ui::open_with(|repo| {
            std::fs::File::create(repo.path().join("large.txt"))
                .expect("create an oversized file")
                .set_len(gf_git::FILE_PREVIEW_LIMIT as u64 + 1)
                .expect("set file size");
        })
        .await,
    )
    .await;
    let _ = ui
        .send(Message::Changes(changes::Message::Select(
            "large.txt".into(),
            changes::Group::Untracked,
        )))
        .await;
    let _ = ui.click("[ View ]").await;
    assert!(ui.shows("File exceeds the 2 MiB preview limit"));
    for shortcut in [Shortcut::SelectAll, Shortcut::Copy] {
        let effects = ui
            .send(Message::KeyInput(focus::KeyInput {
                shortcut: Some(shortcut),
                menu: None,
                nav: None,
            }))
            .await;
        assert_eq!(effects.clipboard, 0);
    }
    let _ = ui
        .send(Message::KeyInput(focus::KeyInput {
            shortcut: Some(Shortcut::Escape),
            menu: None,
            nav: None,
        }))
        .await;
    assert!(ui.app.code_dialog.is_none());
}

#[tokio::test]
async fn comparison_checks_the_size_of_the_previous_version() {
    let mut ui = working_tree(
        Ui::open_with(|repo| {
            let path = repo.path().join("large.txt");
            std::fs::File::create(&path)
                .expect("create an oversized file")
                .set_len(gf_git::FILE_PREVIEW_LIMIT as u64 + 1)
                .expect("set file size");
            repo.git(&["add", "--", "large.txt"]);
            std::fs::write(path, "small working tree file").expect("replace file");
        })
        .await,
    )
    .await;
    let _ = ui
        .send(Message::Changes(changes::Message::Select(
            "large.txt".into(),
            changes::Group::Unstaged,
        )))
        .await;
    let _ = ui.click("[ View ]").await;
    assert!(ui.shows("File exceeds the 2 MiB preview limit"));
}

#[tokio::test]
async fn view_file_loads_the_version_for_each_changes_group() {
    let mut ui = working_tree(
        Ui::open_with(|r| {
            r.write("a.txt", "staged version\n");
            r.git(&["add", "--", "a.txt"]);
            if cfg!(unix) {
                r.write("1:staged.txt", "literal index path\n");
                r.git(&["add", "--", "1:staged.txt"]);
            }
            r.write("a.txt", "worktree version\n");
            r.write("worktree-deleted.txt", "indexed before deletion\n");
            r.git(&["add", "--", "worktree-deleted.txt"]);
            std::fs::remove_file(r.path().join("worktree-deleted.txt")).expect("remove file");
            r.git(&["rm", "--", "src/lib.rs"]);
        })
        .await,
    )
    .await;
    for (path, group, line) in [
        ("a.txt", changes::Group::Staged, "staged version"),
        ("1:staged.txt", changes::Group::Staged, "literal index path"),
        ("a.txt", changes::Group::Unstaged, "worktree version"),
        ("c.txt", changes::Group::Untracked, "untracked"),
        (
            "worktree-deleted.txt",
            changes::Group::Unstaged,
            "indexed before deletion",
        ),
        ("src/lib.rs", changes::Group::Staged, "println!"),
    ] {
        if !cfg!(unix) && path == "1:staged.txt" {
            continue;
        }
        let _ = ui
            .send(Message::Changes(changes::Message::Select(
                path.into(),
                group,
            )))
            .await;
        assert!(!ui.shows("[ blame ]"));
        assert!(!ui.shows("[ file history ]"));
        let opened = ui.click("[ View ]").await;
        assert!(
            opened
                .messages
                .iter()
                .any(|m| m == "LocalDiff(ViewChanges)")
        );
        assert!(!opened.repo_changed);
        assert!(ui.shows("  1"), "{path} should have a code gutter");
        let viewer = ui.app.code_dialog.as_mut().expect("file dialog");
        let _ = viewer.update(code_view::Message::SelectAll);
        assert!(
            matches!(viewer.update(code_view::Message::Copy).output,
                Some(code_view::Output::Copy(text)) if text.contains(line)),
            "{path} in {group:?} should contain {line:?}"
        );
        let split = ui.click("[ Side by side ]").await;
        assert!(
            split.screen_changed,
            "comparison should open for {path} in {group:?}"
        );
        assert!(!split.repo_changed);
        let _ = ui
            .send(Message::CodeDialog(code_view::Message::Close))
            .await;
    }
}

#[tokio::test]
async fn local_view_shows_removed_lines_but_copies_the_current_file() {
    let mut ui = a_txt_shown().await;
    let opened = ui.click("[ View ]").await;
    assert!(!opened.repo_changed);
    let labels = ui.targets();
    let removed = labels
        .iter()
        .find(|t| t.label == "three")
        .expect("removed text");
    let marker = labels
        .iter()
        .find(|t| t.label == "−" && t.at.x < removed.at.x && (t.at.y - removed.at.y).abs() < 1.0)
        .expect("removed line marker");
    assert!(
        labels.iter().any(|t| {
            t.label == "+" && (t.at.x - marker.at.x).abs() < 1.0 && t.at.y > marker.at.y
        })
    );
    assert!(ui.shows("  4"), "unchanged lines are included");
    let viewer = ui.app.code_dialog.as_mut().expect("file dialog");
    let _ = viewer.update(code_view::Message::SelectAll);
    assert!(matches!(
        viewer.update(code_view::Message::Copy).output,
        Some(code_view::Output::Copy(text)) if text == "one\ntwo\nchanged\nfour\n"
    ));
}

#[tokio::test]
async fn file_find_searches_removed_text_and_both_comparison_panes() {
    let mut ui = a_txt_shown().await;
    let _ = ui.click("[ View ]").await;
    let hint = ui.click("Find in file ⌘F").await;
    assert!(hint.messages.is_empty(), "the shortcut hint is plain text");
    let press = |key: keyboard::Key, code, modifiers| {
        global_event(
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Code(code),
                modifiers,
                text: None,
                location: keyboard::Location::Standard,
                repeat: false,
            }),
            event::Status::Ignored,
            window::Id::unique(),
        )
        .expect("file search key")
    };
    use keyboard::Modifiers as Mods;
    use keyboard::key::{Code, Key, Named};
    let open = || press(Key::Character("ㄹ".into()), Code::KeyF, Mods::COMMAND);
    let escape = || press(Key::Named(Named::Escape), Code::Escape, Mods::empty());
    let next = || press(Key::Named(Named::Enter), Code::Enter, Mods::empty());
    let previous = || press(Key::Named(Named::Enter), Code::Enter, Mods::SHIFT);
    let command_next = || press(Key::Character("ㅎ".into()), Code::KeyG, Mods::COMMAND);
    let opened = ui.send(open()).await;
    assert!(
        ui.app
            .code_dialog
            .as_ref()
            .is_some_and(code_view::State::find_is_open)
    );
    assert_eq!(opened.widget_ops, 2);
    {
        let mut sim = ui.simulator();
        let field_id = iced::advanced::widget::Id::from(code_view::find::FIELD_ID);
        let field = sim
            .find(move |candidate: Candidate<'_>| match candidate {
                Candidate::TextInput { id, bounds, .. } if id == Some(&field_id) => Some(bounds),
                _ => None,
            })
            .expect("search input");
        let filename = sim
            .find(move |candidate: Candidate<'_>| match candidate {
                Candidate::Text {
                    content: "a.txt",
                    bounds,
                    ..
                } if (bounds.x - field.x).abs() < 1.0 => Some(bounds),
                _ => None,
            })
            .expect("filename in the file viewer");
        assert!(field.y >= filename.y + filename.height);
        assert!(field.width > WINDOW.width * 0.75);
    }
    let find = |query: &str| Message::CodeDialog(code_view::Message::FindQuery(query.into()));
    let removed = ui.send(find("THREE")).await;
    assert!(ui.shows("1 of 1"));
    assert_eq!(removed.widget_ops, 1);
    let _ = ui.send(find("ONE")).await;
    assert!(ui.shows("1 of 1"));

    let split = ui.click("[ Side by side ]").await;
    assert!(!split.repo_changed);
    assert!(ui.shows("1 of 2"));
    let _ = ui.send(previous()).await;
    assert!(ui.shows("2 of 2"));
    let _ = ui.send(next()).await;
    assert!(ui.shows("1 of 2"));
    let _ = ui.send(command_next()).await;
    assert!(ui.shows("2 of 2"));
    let _ = ui.send(find("changed")).await;
    assert!(ui.shows("1 of 1"));
    let _ = ui.send(find("THREE")).await;
    assert!(ui.shows("1 of 1"));
    let _ = ui.send(find("absent")).await;
    assert!(ui.shows("0 of 0"));

    let _ = ui.send(escape()).await;
    assert!(ui.app.code_dialog.is_some());
    assert!(!ui.shows("0 of 0"));
    let _ = ui.send(open()).await;
    assert!(ui.shows("0 of 0"), "a reopened search keeps its query");
    let _ = ui.send(escape()).await;
    let _ = ui.send(escape()).await;
    assert!(ui.app.code_dialog.is_none());
}

#[tokio::test]
async fn side_by_side_aligns_replacements_and_copies_the_selected_side() {
    let mut ui = working_tree(
        Ui::open_with(|r| {
            r.write("a.txt", "one\ntwo\nreplacement\ninserted\nfour\n");
        })
        .await,
    )
    .await;
    let _ = ui
        .send(Message::Changes(changes::Message::Select(
            "a.txt".into(),
            changes::Group::Unstaged,
        )))
        .await;
    let _ = ui.click("[ View ]").await;
    let initial_close = code_close_target(&ui).at.x;
    let _ = ui.click("[ Side by side ]").await;
    assert!((code_close_target(&ui).at.x - initial_close).abs() < 1.0);
    assert!(!ui.shows("[ Inline ]"));

    let find_line = |label, pane| {
        ui.targets().into_iter().find(|t| {
            t.label == label && ui.press(t.at, mouse::Button::Left).iter().any(|m| {
                matches!(m, Message::CodeDialog(code_view::Message::Pressed(side, ..)) if *side == pane)
            })
        }).expect("numbered line in comparison pane")
    };
    let before = find_line("  4", code_view::Pane::Before);
    let after = find_line("  5", code_view::Pane::After);
    assert!((before.at.y - after.at.y).abs() < 1.0);
    let center = WINDOW.width / 2.0;
    let gutter = ui.app.tokens.cols(6);
    assert!((center - gutter..center).contains(&before.at.x));
    assert!((center..center + gutter).contains(&after.at.x));
    for (target, expected) in [
        (before, "one\ntwo\nthree\nfour\n"),
        (after, "one\ntwo\nreplacement\ninserted\nfour\n"),
    ] {
        let clicked = ui.click_at(&target, mouse::Button::Left).await;
        assert!(!clicked.repo_changed);
        let viewer = ui.app.code_dialog.as_mut().expect("file dialog");
        let _ = viewer.update(code_view::Message::SelectAll);
        assert!(matches!(viewer.update(code_view::Message::Copy).output,
            Some(code_view::Output::Copy(text)) if text == expected));
    }
    assert!(ui.click("[ Side by side ]").await.screen_changed);
    assert!((code_close_target(&ui).at.x - initial_close).abs() < 1.0);
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_file_diff() {
    let silent = sweep("a.txt diff", a_txt_shown).await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_selected_lines() {
    let silent = sweep("selected lines", drag_selected).await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_rebase_editor() {
    let silent = sweep("rebase editor", || async {
        let mut ui = Ui::open_with(|r| {
            r.git(&["reset", "-q", "--hard"]);
            r.git(&["clean", "-qfd"]);
        })
        .await;
        let _ = ui.right_click("first").await;
        let _ = ui.click("rebase -i from here").await;
        assert!(ui.app.rebase_editor().is_some(), "the rebase editor opens");
        ui
    })
    .await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_left_changes() {
    let silent = sweep("left changes", || async {
        let ui = Ui::open_with(|r| {
            r.git(&["reset", "-q", "--hard"]);
            r.git(&["clean", "-qfd"]);
            r.write("a.txt", "left behind\n");
            r.git(&["stash", "push", "-q", "-m", "gf: left on main"]);
        })
        .await;
        assert!(
            ui.shows("left on main") || ui.shows("left"),
            "the banner shows"
        );
        ui
    })
    .await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

async fn drag_selected() -> Ui {
    let mut ui = a_txt_shown().await;
    ui.settle();
    // The first and last diff lines: pressed from each end, not every
    // text on screen (each press lays the window out again).
    let is_line = |t: &&Target| {
        ui.press(t.at, mouse::Button::Left)
            .iter()
            .any(|m| matches!(m, Message::LocalDiff(diff_view::Message::LineClicked(..))))
    };
    let targets = ui.targets();
    let (first, last) = (
        targets.iter().find(is_line).expect("diff lines"),
        targets.iter().rev().find(is_line).expect("diff lines"),
    );
    let fx = ui.drag(first.at, last.at).await;
    // Unmeasured while a sweep gets back here (checked on its first build).
    assert!(
        !ui.measure || fx.screen_changed,
        "dragging selects lines: {fx:?}"
    );
    ui
}

#[tokio::test]
async fn a_clean_working_tree_disables_stash() {
    let mut ui = Ui::open_with(|r| {
        r.git(&["reset", "-q", "--hard"]);
        r.git(&["clean", "-qfd"]);
    })
    .await;
    let stash = ui
        .targets()
        .into_iter()
        .find(|t| t.label == "[ ≡ stash ]")
        .expect("stash button");
    assert!(ui.press(stash.at, mouse::Button::Left).is_empty());
    let _ = ui.send(Message::Toolbar(toolbar::Message::Stash)).await;
    assert!(ui.app.dialog.is_none());
    assert!(ui.flash().contains("nothing to stash"));
}

#[tokio::test]
async fn new_branch_defaults_to_head_and_can_use_the_selected_ref() {
    let mut ui = Ui::open().await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Select(
            sidebar::Selection::Ref("refs/heads/feature".into()),
        )))
        .await;
    for (name, action, option, start) in [
        ("from-head", sidebar::Action::NewBranch, None, "HEAD"),
        (
            "from-feature",
            sidebar::Action::NewBranch,
            Some(1),
            "feature",
        ),
        ("from-menu", sidebar::Action::BranchHere, None, "feature"),
    ] {
        let _ = ui
            .send(Message::Sidebar(sidebar::Message::Act(action)))
            .await;
        let _ = ui
            .send(Message::Dialog(dialog::Message::Field(0, name.into())))
            .await;
        let _ = ui.send(Message::Dialog(dialog::Message::Check(0))).await;
        if let Some(i) = option {
            let _ = ui.send(Message::Dialog(dialog::Message::Choose(i))).await;
        }
        let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
        let repo = &ui.fixture.repo;
        assert_eq!(
            repo.git(&["rev-parse", name]),
            repo.git(&["rev-parse", start])
        );
    }
}

#[tokio::test]
async fn remote_checkout_selects_the_current_tracking_branch_without_switching() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["branch", "-m", "work/main"]);
    })
    .await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Toggle(
            sidebar::Section::Branches,
        )))
        .await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Toggle(
            sidebar::Section::Remotes,
        )))
        .await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Filter(
            "origin/main".into(),
        )))
        .await;
    let _ = ui.click("main").await;
    let fx = ui
        .send(Message::Sidebar(sidebar::Message::Act(
            sidebar::Action::Checkout,
        )))
        .await;

    assert!(!fx.repo_changed);
    assert!(ui.app.dialog.is_none());
    assert_eq!(
        ui.app.active_tab().expect("tab").history.revision(),
        Some("refs/heads/work/main")
    );
    assert_eq!(
        ui.fixture.repo.git(&["branch", "--show-current"]).trim(),
        "work/main"
    );
    assert!(ui.targets().iter().any(|t| t.label == "main"));
}

#[tokio::test]
async fn remote_checkout_selects_a_branch_checked_out_in_a_linked_worktree() {
    let folder = tempfile::tempdir().expect("worktree parent");
    let path = folder.path().join("linked");
    let mut ui = Ui::open_with(|repo| {
        repo.git(&[
            "worktree",
            "add",
            "-q",
            path.to_str().expect("path"),
            "feature",
        ]);
    })
    .await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Select(
            sidebar::Selection::Ref("refs/remotes/origin/feature".into()),
        )))
        .await;
    let fx = ui
        .send(Message::Sidebar(sidebar::Message::Act(
            sidebar::Action::Checkout,
        )))
        .await;

    assert!(!fx.repo_changed);
    assert!(ui.app.dialog.is_none());
    assert_eq!(
        ui.app.active_tab().expect("tab").history.revision(),
        Some("refs/heads/feature")
    );
    assert_eq!(
        ui.fixture.repo.git(&["branch", "--show-current"]).trim(),
        "main"
    );
}

#[tokio::test]
async fn remote_checkout_switches_to_an_existing_tracking_branch() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["reset", "--hard"]);
        repo.git(&["clean", "-fd"]);
    })
    .await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Select(
            sidebar::Selection::Ref("refs/remotes/origin/feature".into()),
        )))
        .await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Act(
            sidebar::Action::Checkout,
        )))
        .await;

    assert!(ui.app.dialog.is_none());
    assert_eq!(
        ui.fixture.repo.git(&["branch", "--show-current"]).trim(),
        "feature"
    );
}

async fn checkout_fixture() -> Ui {
    Ui::open_with(|repo| {
        repo.git(&["reset", "--hard"]);
        repo.git(&["clean", "-fd"]);
        repo.git(&["branch", "other"]);
    })
    .await
}

fn selected_checkout() -> Message {
    Message::Sidebar(sidebar::Message::Act(sidebar::Action::Checkout))
}

#[tokio::test]
async fn refs_allows_browsing_but_blocks_checkout_until_the_new_snapshot() {
    let mut ui = checkout_fixture().await;
    let _ = ui.click("feature").await;
    let switch = ui.app.update(selected_checkout());
    assert!(ui.app.checkout_pending());
    assert!(ui.shows("Switching…"), "target row shows progress");
    assert_eq!(
        ui.app.active_tab().expect("tab").head_label().as_deref(),
        Some("main")
    );

    let _ = ui.click("other").await;
    assert_eq!(
        ui.app.active_tab().expect("tab").history.revision(),
        Some("refs/heads/other")
    );
    let enter = ui
        .send(Message::KeyInput(focus::KeyInput {
            shortcut: Some(Shortcut::Enter),
            menu: None,
            nav: Some(Nav::plain(Key::Enter)),
        }))
        .await;
    assert!(!enter.repo_changed, "Enter cannot launch a second switch");
    let select = Message::Sidebar(sidebar::Message::Select(sidebar::Selection::Ref(
        "refs/heads/other".into(),
    )));
    let double = ui.run(vec![select.clone(), select], false).await;
    assert!(
        !double.repo_changed,
        "a double-click cannot launch a second switch"
    );
    let _ = ui.right_click("other").await;
    assert!(ui.app.menu.is_some());
    assert!(
        !ui.shows("switch your files to it"),
        "checkout is unavailable in the menu"
    );
    let _ = ui.send(Message::Key(Shortcut::Escape)).await;
    assert!(!ui.send(selected_checkout()).await.repo_changed);

    let mut actions = runtime::task::into_stream(switch).expect("switch task");
    let done = loop {
        match actions.next().await.expect("switch completion") {
            Action::Output(done @ Message::OpDone { .. }) => break done,
            _ => continue,
        }
    };
    assert_eq!(
        ui.fixture.repo.git(&["branch", "--show-current"]).trim(),
        "feature"
    );
    let tab = ui.app.active_tab_mut().expect("tab");
    let id = tab.repo.id();
    let previous = tab.snapshot.clone().expect("old snapshot");
    tab.loading = true;
    let _queued = ui.app.update(done);
    assert!(
        ui.app.checkout_pending(),
        "git finished while the old read was in flight"
    );

    let refresh = ui.app.update(Message::Loaded(id, Ok(previous)));
    assert!(
        ui.app.checkout_pending(),
        "the old read must not release the guard"
    );
    assert!(!ui.send(selected_checkout()).await.repo_changed);
    let mut actions = runtime::task::into_stream(refresh).expect("post-checkout refresh");
    let loaded = loop {
        match actions.next().await.expect("snapshot completion") {
            Action::Output(loaded @ Message::Loaded(..)) => break loaded,
            _ => continue,
        }
    };
    let _ = ui.send(loaded).await;
    assert!(!ui.app.checkout_pending());
    assert!(!ui.shows("Switching…"));
    assert_eq!(
        ui.app.active_tab().expect("tab").head_label().as_deref(),
        Some("feature")
    );
    let _ = ui.send(selected_checkout()).await;
    assert_eq!(
        ui.fixture.repo.git(&["branch", "--show-current"]).trim(),
        "other"
    );
}

#[tokio::test]
async fn closing_and_reopening_the_tab_does_not_drop_a_running_checkout() {
    let mut ui = checkout_fixture().await;
    let _ = ui.click("feature").await;
    let task = ui.app.update(selected_checkout());
    let original = ui.app.active_tab().expect("tab").repo.id();
    let _ = ui.send(Message::Tabs(tabs::Message::Close(0))).await;
    let other = Fixture::new();
    let other_path = other.repo.path().to_path_buf();
    ui.others.push(other);
    let _ = ui.send(Message::Open(other_path)).await;
    assert!(
        !ui.app.checkout_pending(),
        "other worktrees can still switch"
    );
    let _ = ui.send(Message::Open(ui.fixture.repo.path().into())).await;
    assert_ne!(
        ui.app.active_tab().expect("reopened tab").repo.id(),
        original
    );
    assert!(ui.app.checkout_pending());

    let mut actions = runtime::task::into_stream(task).expect("switch task");
    let done = loop {
        match actions.next().await.expect("switch completion") {
            Action::Output(done @ Message::OpDone { .. }) => break done,
            _ => continue,
        }
    };
    let _ = ui.send(done).await;
    assert!(!ui.app.checkout_pending());
    let tab = ui.app.active_tab().expect("reopened tab");
    assert_eq!(tab.head_label().as_deref(), Some("feature"));
    assert!(
        tab.undo.next(false).is_some(),
        "the switch is still undoable"
    );
}

#[tokio::test]
async fn failed_checkout_clears_progress_and_allows_retry() {
    let mut ui = checkout_fixture().await;
    let _ = ui.click("feature").await;
    let lock = ui.fixture.repo.path().join(".git/index.lock");
    std::fs::File::create(&lock).expect("simulate another git process holding the lock");
    let switch = ui.send(selected_checkout()).await;
    assert!(!switch.timed_out);
    assert!(!ui.app.checkout_pending());
    assert!(!ui.shows("Switching…"));
    assert!(ui.app.dialog.is_some(), "Git's error is shown");
    assert_eq!(
        ui.fixture.repo.git(&["branch", "--show-current"]).trim(),
        "main"
    );
    std::fs::remove_file(lock).expect("release the fixture lock");
    let _ = ui.send(Message::Key(Shortcut::Escape)).await;
    let _ = ui.send(selected_checkout()).await;
    assert_eq!(
        ui.fixture.repo.git(&["branch", "--show-current"]).trim(),
        "feature"
    );
}

#[tokio::test]
async fn refs_can_delete_a_range_after_review_and_keep_checked_out_branches() {
    let linked = tempfile::tempdir().expect("linked worktree parent");
    let worktree = linked.path().join("worktree");
    let mut ui = Ui::open_with(|repo| {
        for name in ["aaa", "bbb", "held"] {
            repo.git(&["branch", name]);
        }
        repo.git(&["worktree", "add", worktree.to_str().expect("path"), "held"]);
    })
    .await;
    let _ = ui.click("aaa").await;
    let _ = ui
        .send(Message::Modifiers(keyboard::Modifiers::SHIFT))
        .await;
    let _ = ui.click("main").await;
    let _ = ui
        .send(Message::Modifiers(keyboard::Modifiers::empty()))
        .await;
    assert!(ui.shows("[ Delete 5… ]"));
    let _ = ui.right_click("bbb").await;
    assert!(ui.shows("5 branches selected"));
    let _ = ui.click("delete selected branches").await;
    for entry in [
        "Local: aaa",
        "Local: bbb",
        "Local: feature",
        "Keep main (checked out)",
        "Keep held (checked out)",
    ] {
        assert!(ui.shows(entry), "review should include {entry}");
    }
    let enter = ui.send(Message::Key(Shortcut::Enter)).await;
    assert!(
        !enter.repo_changed,
        "destructive confirmation needs a click"
    );
    assert!(ui.shows("[ Delete 3 ]"));

    let _ = ui.click("[ Delete 3 ]").await;
    let remaining = ui
        .fixture
        .repo
        .git(&["branch", "--list", "--format=%(refname:short)"]);
    assert_eq!(remaining.lines().collect::<Vec<_>>(), ["held", "main"]);
    let _ = ui.send(Message::Key(Shortcut::Undo)).await;
    let restored = ui
        .fixture
        .repo
        .git(&["branch", "--list", "--format=%(refname:short)"]);
    assert_eq!(
        restored.lines().collect::<Vec<_>>(),
        ["aaa", "bbb", "feature", "held", "main"]
    );
}

#[tokio::test]
async fn branch_batch_deletion_keeps_an_unmerged_branch_and_records_partial_undo() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["reset", "--hard"]);
        repo.git(&["clean", "-fd"]);
        repo.git(&["switch", "-c", "unmerged"]);
        repo.write("only-branch.txt", "unmerged content");
        repo.commit_all("unmerged work");
        repo.git(&["switch", "main"]);
    })
    .await;
    let _ = ui.click("feature").await;
    let _ = ui
        .send(Message::Modifiers(keyboard::Modifiers::COMMAND))
        .await;
    let _ = ui.click("unmerged").await;
    let _ = ui
        .send(Message::Modifiers(keyboard::Modifiers::empty()))
        .await;
    let _ = ui.click("[ Delete 2… ]").await;
    let _ = ui.click("[ Delete 2 ]").await;
    assert!(ui.shows("Some selected branches could not be deleted"));
    let remaining = ui
        .fixture
        .repo
        .git(&["branch", "--list", "--format=%(refname:short)"]);
    assert_eq!(remaining.lines().collect::<Vec<_>>(), ["main", "unmerged"]);
    let _ = ui.send(Message::Key(Shortcut::Escape)).await;
    let _ = ui.send(Message::Key(Shortcut::Undo)).await;
    let restored = ui
        .fixture
        .repo
        .git(&["branch", "--list", "--format=%(refname:short)"]);
    assert_eq!(
        restored.lines().collect::<Vec<_>>(),
        ["feature", "main", "unmerged"]
    );
}

#[tokio::test]
async fn branch_batch_confirms_local_and_remote_destinations_together() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["branch", "remote-only"]);
        repo.git(&["push", "origin", "remote-only"]);
        repo.git(&["tag", "remote-only"]);
        repo.git(&["push", "origin", "refs/tags/remote-only"]);
        repo.git(&["branch", "-D", "remote-only"]);
    })
    .await;
    let _ = ui.click("feature").await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::ToggleBranch(
            "refs/remotes/origin/feature".into(),
        )))
        .await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::ToggleBranch(
            "refs/remotes/origin/remote-only".into(),
        )))
        .await;
    let _ = ui.click("[ Delete 3… ]").await;
    for text in [
        "Local: feature",
        "On remote origin: feature",
        "On remote origin: remote-only",
    ] {
        assert!(ui.shows(text), "review should include {text}");
    }
    let _ = ui.click("[ Delete 3 ]").await;
    assert!(ui.app.dialog.is_none(), "batch succeeded");
    assert!(
        ui.fixture
            .repo
            .git(&["branch", "--list", "feature"])
            .is_empty()
    );
    let refs = ui.fixture.repo.git(&["ls-remote", "origin"]);
    assert!(!refs.contains("refs/heads/feature"));
    assert!(!refs.contains("refs/heads/remote-only"));
    assert!(refs.contains("refs/tags/remote-only"));
}

#[tokio::test]
async fn removing_a_worktree_closes_its_open_tab() {
    let mut ui = Ui::open().await;
    let folder = tempfile::tempdir().expect("worktree parent");
    let path = folder
        .path()
        .canonicalize()
        .map(gf_test_support::without_verbatim_prefix)
        .expect("worktree parent path")
        .join("wt");
    let repo = &ui.fixture.repo;
    repo.git(&[
        "worktree",
        "add",
        "-q",
        path.to_str().expect("path"),
        "feature",
    ]);
    let _ = ui.send(Message::Toolbar(toolbar::Message::Refresh)).await;
    let _ = ui.send(Message::Open(path.clone())).await;
    assert_eq!(ui.app.tabs.len(), 2);
    let _ = ui.send(Message::Tabs(tabs::Message::Select(0))).await;
    let _ = ui
        .send(Message::RefsMode(worktrees::RefsMode::Worktrees))
        .await;
    let card = ui
        .app
        .active_tab()
        .and_then(|t| t.worktrees.cards().iter().find(|c| !c.main))
        .expect("the linked tree's card");
    assert!(card.in_tab, "its tab is marked");
    assert!(card.status.is_some(), "its status was read");
    let _ = ui
        .send(Message::Worktrees(worktrees::Message::Select(path.clone())))
        .await;
    let _ = ui
        .send(Message::Worktrees(worktrees::Message::Act(
            worktrees::Action::Remove,
        )))
        .await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert!(!path.exists());
    assert_eq!(ui.app.tabs.len(), 1);
}

#[tokio::test]
async fn new_worktree_goes_beside_the_repository_and_opens_in_a_tab() {
    let mut ui = Ui::open().await;
    let _ = ui
        .send(Message::RefsMode(worktrees::RefsMode::Worktrees))
        .await;
    let _ = ui.send(Message::Worktrees(worktrees::Message::New)).await;
    let _ = ui
        .send(Message::Dialog(dialog::Message::Field(
            0,
            "agent/one".into(),
        )))
        .await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    let repo = ui.fixture.repo.path().to_path_buf();
    let name = ui.app.tabs[0].name();
    let expected = repo
        .parent()
        .expect("parent")
        .join(format!("{name}-worktrees"))
        .join("agent-one");
    assert!(expected.exists(), "{} was created", expected.display());
    assert_eq!(ui.app.tabs.len(), 2, "and opened in a tab");
    let _ = std::fs::remove_dir_all(expected.parent().expect("root"));
}

#[tokio::test]
async fn a_new_worktree_checks_an_existing_branch_out_instead_of_failing() {
    let mut ui = Ui::open_with(|r| {
        r.git(&["branch", "side"]);
    })
    .await;
    let _ = ui
        .send(Message::RefsMode(worktrees::RefsMode::Worktrees))
        .await;
    let _ = ui.send(Message::Worktrees(worktrees::Message::New)).await;
    let _ = ui
        .send(Message::Dialog(dialog::Message::Field(0, "side".into())))
        .await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    let name = ui.app.tabs[0].name();
    let root = ui
        .fixture
        .repo
        .path()
        .parent()
        .expect("parent")
        .join(format!("{name}-worktrees"));
    let head = ui.fixture.repo.git(&[
        "-C",
        &root.join("side").display().to_string(),
        "branch",
        "--show-current",
    ]);
    assert_eq!(head.trim(), "side", "the existing branch is checked out");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn a_worktrees_branch_merges_into_its_base_in_the_tab_of_the_base() {
    let mut ui = Ui::open_with(|_| {}).await;
    let _ = ui
        .send(Message::RefsMode(worktrees::RefsMode::Worktrees))
        .await;
    let _ = ui.send(Message::Worktrees(worktrees::Message::New)).await;
    let _ = ui
        .send(Message::Dialog(dialog::Message::Field(0, "feat".into())))
        .await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    let name = ui.app.tabs[0].name();
    let root = ui
        .fixture
        .repo
        .path()
        .parent()
        .expect("parent")
        .join(format!("{name}-worktrees"));
    let tree = root.join("feat");
    let dir = tree.display().to_string();
    std::fs::write(tree.join("feat.txt"), "feat\n").expect("write");
    ui.fixture.repo.git(&["-C", &dir, "add", "feat.txt"]);
    ui.fixture
        .repo
        .git(&["-C", &dir, "commit", "-qm", "feat work"]);
    // The new tree opened in its own tab; read the trees again there.
    assert_eq!(ui.app.tabs.len(), 2);
    let _ = ui.send(Message::WorktreePoll).await;
    let card = ui.app.tabs[ui.app.active]
        .worktrees
        .cards()
        .iter()
        .find(|c| c.tree.branch.as_deref() == Some("feat"))
        .expect("feat card")
        .clone();
    let s = card.status.expect("status");
    assert_eq!(
        (s.base.as_deref(), s.ahead_of_base, s.base_guessed),
        (Some("main"), 1, false)
    );
    let _ = ui
        .send(Message::Worktrees(worktrees::Message::Select(
            card.tree.path.clone(),
        )))
        .await;
    let _ = ui
        .send(Message::Worktrees(worktrees::Message::Act(
            worktrees::Action::Merge,
        )))
        .await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert_eq!(ui.app.active, 0, "the merge runs in the tab of main");
    let log = ui.fixture.repo.git(&["log", "--format=%s", "-1", "main"]);
    assert_eq!(log.trim(), "feat work");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn a_base_set_by_hand_merges_in_a_tree_opened_for_it() {
    let mut ui = Ui::open_with(|_| {}).await;
    let _ = ui
        .send(Message::RefsMode(worktrees::RefsMode::Worktrees))
        .await;
    // Two trees, neither opened in a tab.
    for branch in ["develop", "feat"] {
        let _ = ui.send(Message::Worktrees(worktrees::Message::New)).await;
        let _ = ui
            .send(Message::Dialog(dialog::Message::Field(0, branch.into())))
            .await;
        let _ = ui.send(Message::Dialog(dialog::Message::Check(0))).await;
        let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    }
    assert_eq!(ui.app.tabs.len(), 1);
    let name = ui.app.tabs[0].name();
    let root = ui
        .fixture
        .repo
        .path()
        .parent()
        .expect("parent")
        .join(format!("{name}-worktrees"));
    let feat = root.join("feat");
    let dir = feat.display().to_string();
    std::fs::write(feat.join("feat.txt"), "feat\n").expect("write");
    ui.fixture.repo.git(&["-C", &dir, "add", "feat.txt"]);
    ui.fixture
        .repo
        .git(&["-C", &dir, "commit", "-qm", "feat work"]);
    let _ = ui.send(Message::WorktreePoll).await;
    let path = ui.app.tabs[0]
        .worktrees
        .cards()
        .iter()
        .find(|c| c.tree.branch.as_deref() == Some("feat"))
        .expect("feat card")
        .tree
        .path
        .clone();
    let _ = ui
        .send(Message::Worktrees(worktrees::Message::Select(path)))
        .await;

    // feat came from main (HEAD then); say it goes into develop.
    let _ = ui
        .send(Message::Worktrees(worktrees::Message::Act(
            worktrees::Action::SetBase,
        )))
        .await;
    let _ = ui
        .send(Message::Dialog(dialog::Message::Field(0, "develop".into())))
        .await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert_eq!(
        ui.fixture
            .repo
            .git(&["config", "--get", "branch.feat.gf-base"])
            .trim(),
        "develop"
    );
    let _ = ui.send(Message::WorktreePoll).await;
    let _ = ui
        .send(Message::Worktrees(worktrees::Message::Act(
            worktrees::Action::Merge,
        )))
        .await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert_eq!(ui.app.tabs.len(), 2, "develop's tree opened for the merge");
    let log = ui
        .fixture
        .repo
        .git(&["log", "--format=%s", "-1", "develop"]);
    assert_eq!(log.trim(), "feat work");
    let main = ui.fixture.repo.git(&["log", "--format=%s", "-1", "main"]);
    assert_ne!(main.trim(), "feat work", "main is left alone");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn a_branch_without_commits_stays_visible_after_renaming_head() {
    let repo = TestRepo::new();
    let mut ui = Ui::welcome().await;
    let _ = ui.send(Message::Open(repo.path().to_path_buf())).await;
    assert!(
        ui.targets()
            .iter()
            .any(|t| t.label.contains("main (no commits yet)"))
    );
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Act(
            sidebar::Action::NewBranch,
        )))
        .await;
    assert!(
        !ui.targets()
            .iter()
            .any(|t| t.label.contains("switch to the new branch"))
    );
    let _ = ui
        .send(Message::Dialog(dialog::Message::Field(0, "dev".into())))
        .await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "dev");
    assert!(
        ui.targets()
            .iter()
            .any(|t| t.label.contains("dev (no commits yet)"))
    );
}

#[tokio::test]
async fn blame_truncates_a_wide_author_to_the_column_width() {
    let mut ui = Ui::open_with(|r| {
        r.git(&[
            "commit",
            "--author=가나다라마바사아 <wide@example.com>",
            "-m",
            "wide author",
        ]);
    })
    .await;
    let _ = ui.click("wide author").await;
    assert!(!ui.shows("[ file history ]"));
    assert!(!ui.shows("[ blame ]"));
    let _ = ui.right_click("b.txt").await;
    let _ = ui.click("blame").await;
    assert!(ui.targets().iter().any(|t| t.label == "가나다라마…"));
}

#[tokio::test]
async fn blame_search_finds_worktree_lines_and_keeps_commit_navigation() {
    use crate::features::blame_view::Message as Blame;
    use keyboard::key::Code;

    let mut ui = a_txt_shown().await;
    let _ = ui.right_click("a.txt").await;
    let _ = ui.click("blame").await;
    assert!(ui.shows("Find in file ⌘F"));

    let command = |key: &str, code, modifiers| {
        let key = keyboard::Key::Character(key.into());
        global_event(
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Code(code),
                modifiers,
                text: None,
                location: keyboard::Location::Standard,
                repeat: false,
            }),
            event::Status::Ignored,
            window::Id::unique(),
        )
        .expect("search shortcut")
    };
    let mods = keyboard::Modifiers::COMMAND;
    assert_eq!(ui.send(command("ㄹ", Code::KeyF, mods)).await.widget_ops, 2);
    let query = |text: &str| Message::Blame(Blame::FindQuery(text.into()));
    assert_eq!(ui.send(query("CHANGED")).await.widget_ops, 1);
    assert!(ui.shows("1 of 1"));
    let _ = ui.send(query("O")).await;
    assert!(ui.shows("1 of 3"));
    let _ = ui
        .send(command("ㅎ", Code::KeyG, mods | keyboard::Modifiers::SHIFT))
        .await;
    assert!(ui.shows("3 of 3"));
    let _ = ui.send(command("ㅎ", Code::KeyG, mods)).await;
    assert!(ui.shows("1 of 3"));
    let _ = ui.send(query("changed")).await;

    let selected = ui.click("    4").await;
    assert!(
        selected
            .messages
            .iter()
            .any(|message| message.starts_with("Blame(Select("))
    );
    assert!(
        ui.shows("1 of 1"),
        "the search survives following a blame row"
    );
    let _ = ui
        .send(Message::KeyInput(focus::KeyInput {
            shortcut: Some(Shortcut::Escape),
            nav: None,
            menu: None,
        }))
        .await;
    assert!(ui.shows("Find in file ⌘F"), "Escape closes the search only");
}

#[tokio::test]
async fn diff_exposes_copy_only_for_a_selection() {
    let ui = a_txt_shown().await;
    assert!(!ui.targets().iter().any(|t| t.label == "[ copy ]"));
    let ui = drag_selected().await;
    assert!(
        ui.targets()
            .iter()
            .any(|t| t.label == crate::platform::keys::local("[ copy ⌘C ]".into()))
    );
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_conflict_resolver() {
    let silent = sweep("conflict resolver", || async {
        let mut ui = working_tree(
            Ui::open_with(|r| {
                diverged(r);
                let _ = r.git_output(&["merge", "feature"]);
            })
            .await,
        )
        .await;
        let _ = ui
            .send(Message::Changes(changes::Message::Select(
                "a.txt".into(),
                changes::Group::Conflicts,
            )))
            .await;
        // One choice made: Apply can run.
        let _ = ui
            .send(Message::Conflict(
                crate::features::conflict_view::Message::Choose(0, gf_git::conflict::Choice::Ours),
            ))
            .await;
        ui
    })
    .await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

#[tokio::test]
#[ignore = "UI sweep, minutes: cargo test -p gf-app -- --ignored sweep_"]
async fn sweep_rebase_resolved() {
    let silent = sweep("rebase resolved", || async {
        working_tree(
            Ui::open_with(|r| {
                diverged(r);
                let _ = r.git_output(&["rebase", "feature"]);
                r.write("a.txt", "resolved\n");
                r.git(&["add", "a.txt"]);
            })
            .await,
        )
        .await
    })
    .await;
    assert!(
        silent.is_empty(),
        "clicks that did nothing:\n{}",
        report(&silent)
    );
}

#[tokio::test]
async fn clicking_a_tab_in_the_switcher_shows_it() {
    let mut ui = Ui::open().await;
    let other = Fixture::new();
    let path = other.repo.path().to_path_buf();
    let name = path
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    ui.others.push(other);
    let _ = ui.send(Message::Open(path)).await;
    let _ = ui.send(Message::Tabs(tabs::Message::Select(0))).await;
    // ⌃ held, ⌃⇥ pressed: the switcher shows over the window.
    let _ = ui.send(Message::Modifiers(keyboard::Modifiers::CTRL)).await;
    let _ = ui
        .send(Message::Key(Shortcut::CycleTab { back: false }))
        .await;
    assert!(ui.app.switcher.is_some());
    ui.settle();
    // The switcher's row (the last label with the name is on top).
    let row = ui
        .targets()
        .into_iter()
        .rfind(|t| t.label.contains(&name))
        .expect("the other tab in the switcher");
    let _ = ui.click_at(&row, mouse::Button::Left).await;
    assert_eq!(ui.app.active, 1, "the clicked tab is shown");
    assert!(ui.app.switcher.is_none());
}

#[tokio::test]
async fn dragging_the_divider_resizes_the_file_list_and_remembers_it() {
    let mut ui = working_tree(Ui::open().await).await;
    ui.settle();
    let diff = ui
        .targets()
        .into_iter()
        .find(|t| t.label.starts_with("━ LOCAL CHANGES"))
        .expect("the diff panel");
    // The divider is just left of the diff panel: find where a press
    // starts a resize.
    let y = diff.at.y + 40.0;
    let x = (0..40)
        .map(|dx| diff.at.x - 60.0 - dx as f32)
        .chain((0..200).map(|dx| diff.at.x - dx as f32))
        .find(|&x| {
            ui.press(Point::new(x, y), mouse::Button::Left)
                .iter()
                .any(|m| matches!(m, Message::ResizeListStart))
        })
        .expect("the divider");
    let before = ui.app.settings.appearance.list_cols;
    let fx = ui
        .drag_through(Point::new(x, y), Point::new(x + 80.0, y))
        .await;
    let after = ui.app.settings.appearance.list_cols;
    assert!(after > before, "{before} → {after}: {fx:?}");
    let saved = gf_config::Settings::load(&ui.app.paths.settings_file()).expect("saved");
    assert!((saved.appearance.list_cols - after).abs() < f32::EPSILON);

    // A double-click puts it back.
    ui.settle();
    let at = Point::new(x + 80.0, y);
    let messages = {
        let mut sim = ui.simulator();
        sim.point_at(at);
        let click = [
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ];
        let _ = sim.simulate(
            std::iter::once(iced::Event::Mouse(mouse::Event::CursorMoved {
                position: at,
            }))
            .chain(click.clone())
            .chain(click),
        );
        sim.into_messages().collect()
    };
    let _ = ui.run(messages, false).await;
    assert!(
        (ui.app.settings.appearance.list_cols - gf_config::DEFAULT_LIST_COLS).abs() < f32::EPSILON
    );
}

// Fast checks for buttons the sweep found doing nothing (these run with
// every `cargo test`; the sweeps above are `--ignored`).

#[tokio::test]
async fn settings_from_the_welcome_screen_apply_and_remember_the_theme() {
    let mut ui = Ui::welcome().await;
    let _ = ui.click("[ Settings ⌘, ]").await;
    assert!(ui.app.settings_open);
    assert!(ui.shows(concat!("v", env!("CARGO_PKG_VERSION"))));
    for name in ["gf-light", "cd-dark", "cd-light"] {
        let _ = ui.click(name).await;
        assert_eq!(ui.app.tokens.name, name);
    }
    assert!(ui.app.settings_open);

    let selected = ui
        .targets()
        .into_iter()
        .find(|target| target.label == "cd-light")
        .expect("selected theme");
    assert!(ui.press(selected.at, mouse::Button::Left).is_empty());

    let saved = Settings::load(&ui.app.paths.settings_file()).expect("saved settings");
    let (reopened, _) = App::boot(ui.app.paths.clone(), saved);
    assert_eq!(reopened.tokens.name, "cd-light");

    let _ = ui.click("[ Done Esc ]").await;
    assert!(!ui.app.settings_open);
}

#[tokio::test]
async fn settings_keys_change_the_theme_without_navigating_the_repository() {
    let mut ui = Ui::open().await;
    let active = ui.app.pane();
    let _ = ui.send(Message::Key(Shortcut::Settings)).await;
    assert!(ui.app.settings_open);
    let _ = ui
        .send(Message::KeyInput(focus::KeyInput {
            shortcut: None,
            menu: None,
            nav: Some(Nav::plain(Key::Up)),
        }))
        .await;
    assert_eq!(ui.app.tokens.name, "cd-light");
    assert_eq!(ui.app.pane(), active);

    let _ = ui.send(Message::Key(Shortcut::Escape)).await;
    assert!(!ui.app.settings_open);
    assert_eq!(ui.app.tokens.name, "cd-light");
}

#[tokio::test]
async fn history_and_command_log_can_be_collapsed_and_reopened_by_clicking() {
    let mut ui = Ui::open().await;
    ui.app.focus_pane(Pane::History);
    assert!(!ui.app.settings.appearance.show_command_log);
    for (id, arg, read_only) in [(1, "status", false), (2, "rev-parse", true)] {
        let _ = ui
            .send(Message::Core(CoreEvent::Command(gf_core::CommandEvent {
                id,
                repo: None,
                phase: gf_core::CommandPhase::Started {
                    argv: vec!["git".into(), arg.into()],
                    read_only,
                },
            })))
            .await;
    }
    ui.app
        .log
        .note(NoteKind::Info, "keep this log entry", Vec::new());
    assert!(ui.targets().iter().any(|t| t.label == "git status"));
    assert!(!ui.targets().iter().any(|t| t.label.contains("rev-parse")));
    assert!(
        !ui.targets()
            .iter()
            .any(|t| t.label.contains("keep this log entry"))
    );
    let _ = ui.click("git status").await;
    assert!(ui.app.settings.appearance.show_command_log);
    let _ = ui.click("▴").await;
    assert!(!ui.app.panes().contains(&Pane::History));
    assert_eq!(ui.app.pane(), Pane::Files);
    let collapse_log = ui
        .targets()
        .into_iter()
        .filter(|t| t.label == "▾")
        .max_by(|a, b| a.at.x.total_cmp(&b.at.x))
        .expect("collapse log button at the right edge");
    let _ = ui.click_at(&collapse_log, mouse::Button::Left).await;
    assert!(!ui.app.settings.appearance.show_command_log);

    let saved = Settings::load(&ui.app.paths.settings_file()).expect("saved settings");
    assert!(!saved.appearance.show_history);
    assert!(!saved.appearance.show_command_log);

    let _ = ui.click("▾ HISTORY").await;
    assert!(ui.app.panes().contains(&Pane::History));
    assert_eq!(ui.app.pane(), Pane::History);
    let _ = ui.click("▴ COMMAND LOG").await;
    assert!(ui.app.settings.appearance.show_command_log);
    assert!(
        ui.targets()
            .iter()
            .any(|target| target.label.contains("keep this log entry"))
    );
}

#[tokio::test]
async fn clear_is_disabled_while_the_log_has_nothing_to_clear() {
    let mut ui = Ui::welcome().await;
    let _ = ui.click("▴ COMMAND LOG").await;
    ui.settle();
    let clear = ui
        .targets()
        .into_iter()
        .find(|t| t.label == "[ clear ]")
        .expect("clear button");
    assert!(ui.press(clear.at, mouse::Button::Left).is_empty());
    ui.app.log.note(NoteKind::Info, "something", Vec::new());
    assert!(!ui.press(clear.at, mouse::Button::Left).is_empty());
}

#[tokio::test]
async fn refresh_says_it_is_done_even_when_nothing_changed() {
    let mut ui = Ui::open().await;
    let fx = ui.click("[ ↻ refresh").await;
    assert!(ui.flash().contains("refreshed"), "{fx:?}");
}

#[tokio::test]
async fn the_conflicts_button_opens_the_next_conflicted_file() {
    let mut ui = working_tree(
        Ui::open_with(|r| {
            diverged(r);
            let _ = r.git_output(&["merge", "feature"]);
        })
        .await,
    )
    .await;
    // Already in the working tree, where it used to do nothing.
    let _ = ui.click("[ 1 conflict").await;
    let open = ui.app.active_tab().and_then(|t| t.conflict.as_ref());
    assert_eq!(open.map(|c| c.path.clone()), Some(PathBuf::from("a.txt")));
    // The only one is open: it says so.
    let fx = ui.click("[ 1 conflict").await;
    assert!(ui.flash().contains("only conflict"), "{fx:?}");
}

#[tokio::test]
async fn rebase_rows_cannot_move_past_the_ends() {
    let mut ui = Ui::open_with(|r| {
        r.git(&["reset", "-q", "--hard"]);
        r.git(&["clean", "-qfd"]);
    })
    .await;
    let _ = ui.right_click("first").await;
    let _ = ui.click("rebase -i from here").await;
    ui.settle();
    let targets = ui.targets();
    let ups: Vec<&Target> = targets.iter().filter(|t| t.label == "↑").collect();
    let downs: Vec<&Target> = targets.iter().filter(|t| t.label == "↓").collect();
    assert_eq!((ups.len(), downs.len()), (2, 2), "two commits to rebase");
    let press = |t: &Target| ui.press(t.at, mouse::Button::Left).len();
    assert_eq!(press(ups[0]), 0, "the first row has nothing above it");
    assert_eq!(press(downs[1]), 0, "the last row has nothing below it");
    assert_eq!(press(ups[1]), 1);
    assert_eq!(press(downs[0]), 1);
}

#[tokio::test]
async fn commit_button_counts_the_staged_files() {
    let mut ui = working_tree(
        Ui::open_with(|r| {
            r.write("one.txt", "1\n");
            r.write("two.txt", "2\n");
            r.git(&["add", "one.txt", "two.txt"]);
        })
        .await,
    )
    .await;
    let staged = ui
        .app
        .active_tab()
        .and_then(|t| t.snapshot.as_deref())
        .expect("status")
        .staged_count();
    assert!(staged >= 2);
    assert!(ui.shows(&format!("[ Commit {staged} ]")));

    let _ = ui
        .send(Message::Changes(changes::Message::UnstageAll))
        .await;
    ui.settle();
    assert!(ui.shows("[ Commit ]"), "nothing staged: no count");
}

#[tokio::test]
async fn a_flash_does_not_move_the_window_contents() {
    let mut ui = working_tree(Ui::open().await).await;
    let commit = |ui: &Ui| {
        ui.targets()
            .into_iter()
            .find(|t| t.label.starts_with("[ Commit"))
            .expect("commit button")
            .at
    };
    ui.settle();
    let without = commit(&ui);
    ui.app.flash(
        NoteKind::Error,
        "✗ a long message about what went wrong, long enough to need room in the status line",
    );
    assert_eq!(commit(&ui), without, "buttons stay where they were");
}

#[tokio::test]
async fn delayed_rebase_plan_belongs_to_its_original_tab() {
    use crate::features::{dialog, rebase_editor};
    let mut ui = Ui::open_with(|r| {
        r.git(&["reset", "--hard"]);
        r.git(&["clean", "-fd"]);
    })
    .await;
    let other = ui.fixture.repo.duplicate();
    let _ = ui.send(Message::Open(other.path().to_owned())).await;
    let original_head = other.head();
    ui.app.active = 0;
    let base = ui
        .fixture
        .repo
        .git(&["rev-parse", "HEAD~2"])
        .trim()
        .parse()
        .expect("base oid");
    let task = ui.app.open_rebase_editor(base);
    ui.app.active = 1;
    let mut stream = runtime::task::into_stream(task).expect("plan task");
    let Some(Action::Output(done)) = stream.next().await else {
        panic!("expected plan")
    };
    let _ = ui.send(done.clone()).await;
    assert!(ui.app.rebase_editor().is_none(), "B must not show A's plan");
    ui.app.active = 0;
    let editor = ui.app.tabs[0].rebase_editor.as_mut().expect("plan on A");
    editor.items.last_mut().expect("two commits").action = gf_git::TodoAction::Drop;
    let _ = ui
        .send(Message::RebaseEditor(rebase_editor::Message::Start))
        .await;
    assert!(ui.app.dialog.is_some());
    ui.app.active = 1;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert_ne!(ui.fixture.repo.head(), original_head, "A was rewritten");
    assert_eq!(other.head(), original_head);
    assert!(other.git(&["status", "--porcelain"]).is_empty());
    assert_eq!(ui.app.tabs[0].undo.undo.len(), 1);
    assert!(ui.app.tabs[1].undo.undo.is_empty());

    let _ = ui.app.close_tab(0);
    let _ = ui.send(done).await;
    assert!(
        ui.app.rebase_editor().is_none(),
        "closed tab's result is discarded"
    );
}

#[tokio::test]
async fn opening_another_repository_dismisses_the_previous_delete_review() {
    let mut ui = Ui::open().await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::ToggleBranch(
            "refs/heads/feature".into(),
        )))
        .await;
    let _ = ui.click("[ Delete 1… ]").await;
    assert!(ui.app.dialog.is_some());
    let other = Fixture::new();
    let _ = ui.send(Message::Open(other.repo.path().into())).await;
    assert!(
        ui.app.dialog.is_none(),
        "the confirmation targeted the previous repository"
    );
    for repo in [&ui.fixture.repo, &other.repo] {
        assert_eq!(repo.git(&["branch", "--list", "feature"]).trim(), "feature");
    }
}

#[tokio::test]
async fn deleting_a_remapped_remote_branch_preserves_its_local_alias_on_the_remote() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["branch", "alias"]);
        repo.git(&["push", "origin", "alias"]);
        repo.git(&[
            "config",
            "--replace-all",
            "remote.origin.fetch",
            "+refs/heads/feature:refs/remotes/origin/alias",
        ]);
        repo.git(&["fetch", "origin"]);
    })
    .await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::RightClick(
            sidebar::Selection::Ref("refs/remotes/origin/alias".into()),
        )))
        .await;
    let _ = ui
        .send(Message::KeyInput(focus::KeyInput {
            shortcut: None,
            menu: Some(context_menu::Key::Char('d')),
            nav: None,
        }))
        .await;
    assert!(ui.shows("On remote origin: feature"));
    let _ = ui.click("[ Delete 1 ]").await;
    let refs = ui.fixture.repo.git(&["ls-remote", "--heads", "origin"]);
    assert!(!refs.contains("refs/heads/feature"));
    assert!(refs.contains("refs/heads/alias"));
}

#[tokio::test]
async fn late_history_and_blame_results_do_not_replace_newer_requests() {
    let mut ui = Ui::open().await;
    ui.app.settings.appearance.show_history = false;
    let _older = ui.app.open_file_history("a.txt".into(), None);
    let tab = ui.app.active_tab().expect("tab");
    let id = tab.repo.id();
    let old = tab.history_request;
    let label = tab.history.revision().expect("review fixture").to_owned();
    let _newer = ui.app.open_file_history("a.txt".into(), None);
    assert!(ui.app.settings.appearance.show_history);
    let _ = ui.app.update(Message::HistoryLoaded(
        id,
        old,
        label.clone(),
        Err("old".into()),
    ));
    assert_eq!(
        ui.app
            .active_tab()
            .expect("review fixture")
            .history
            .revision(),
        Some(label.as_str())
    );

    let _older = ui.app.open_blame("a.txt".into(), None);
    let old = ui.app.active_tab().expect("review fixture").blame_request;
    let _newer = ui.app.open_blame("a.txt".into(), Some("HEAD".into()));
    let _ = ui.app.update(Message::BlameLoaded(
        id,
        old,
        "a.txt".into(),
        Ok(Vec::new()),
    ));
    assert!(
        ui.app
            .active_tab()
            .expect("review fixture")
            .blame
            .as_ref()
            .expect("review fixture")
            .lines()
            .is_none()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn file_history_preserves_native_names_and_treats_pathspec_magic_literally() {
    use std::os::unix::ffi::OsStrExt;
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(b":(glob)*\xff"));
    let mut ui = Ui::open_with(|repo| {
        // macOS filesystems reject some native byte names that Git trees can contain.
        repo.write("fixture-blob", "unique file history fixture");
        let blob = repo.git(&["hash-object", "-w", "fixture-blob"]);
        let added = repo
            .command()
            .args(["update-index", "--add", "--cacheinfo", "100644", &blob])
            .arg(&path)
            .output()
            .expect("write fixture index");
        assert!(added.status.success(), "{added:?}");
        repo.git(&["commit", "-qm", "literal filename"]);
    })
    .await;
    let task = ui.app.open_file_history(path, None);
    let mut stream = runtime::task::into_stream(task).expect("file history request");
    while let Some(action) = stream.next().await {
        if let Action::Output(Message::HistoryLoaded(_, _, _, result)) = action {
            let history = result.expect("read native path");
            assert_eq!(history.commits.len(), 1);
            assert_eq!(history.commits[0].summary, "literal filename");
            return;
        }
    }
    panic!("missing history result");
}

/// Records `n` commits made outside gf as undo steps, the way a commit
/// in gf records them (HEAD only, undone with `reset --soft`).
async fn record_commits(ui: &mut Ui, n: usize) -> Vec<String> {
    let repo = ui.app.tabs[0].repo.clone();
    let scope = gf_git::Scope::head();
    let mut heads = Vec::new();
    for i in 0..n {
        let before = repo.capture(&scope).await.expect("capture");
        ui.fixture.repo.write(format!("batch{i}.txt"), "x\n");
        heads.push(ui.fixture.repo.commit_all(&format!("batch {i}")));
        let after = repo.capture(&scope).await.expect("capture");
        ui.app.tabs[0].undo.push(crate::undo::Step::new(
            format!("commit: batch {i}"),
            scope.clone(),
            before,
            after,
            gf_git::HeadMode::Soft,
        ));
    }
    heads
}

#[tokio::test]
async fn going_back_several_commits_asks_once_for_all_of_them() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["reset", "--hard"]);
        repo.git(&["clean", "-fd"]);
    })
    .await;
    let start = ui.fixture.repo.head();
    record_commits(&mut ui, 3).await;

    let _ = ui
        .send(Message::UndoList(
            crate::features::undo_list::Message::Toggle,
        ))
        .await;
    // Rows: newest first, so the third row goes back over all three.
    let _ = ui
        .send(Message::UndoList(
            crate::features::undo_list::Message::Pick(2),
        ))
        .await;
    let dialog = ui.app.dialog.as_ref().expect("one confirmation");
    assert_eq!(dialog.title, "Undo 3 steps");
    assert!(dialog.body.iter().any(|l| l.contains("commit: batch 0")));
    assert!(dialog.body.iter().any(|l| l.starts_with("In the end `")));

    let _ = ui.click("[ Undo 3 ⏎ ]").await;
    assert!(ui.app.dialog.is_none(), "no question per step");
    assert_eq!(ui.fixture.repo.head(), start);
    let tab = &ui.app.tabs[0];
    assert_eq!((tab.undo.undo.len(), tab.undo.redo.len()), (0, 3));
    assert!(tab.undo_queue.is_none());
    assert!(ui.flash().contains("undid 3 steps"), "{}", ui.flash());
}

#[tokio::test]
async fn a_batch_stops_at_the_first_step_that_fails() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["reset", "--hard"]);
        repo.git(&["clean", "-fd"]);
    })
    .await;
    // An older step whose tag is gone since: its guard fails.
    let tag_scope = crate::undo::tag("gone");
    let tagged = gf_git::Capture {
        refs: vec![("refs/tags/gone".into(), Some(ui.fixture.repo.head()))],
        ..gf_git::Capture::default()
    };
    ui.app.tabs[0].undo.push(crate::undo::Step::new(
        "create tag gone".into(),
        tag_scope.clone(),
        gf_git::Capture {
            refs: vec![("refs/tags/gone".into(), None)],
            ..gf_git::Capture::default()
        },
        tagged,
        gf_git::HeadMode::Keep,
    ));
    let heads = record_commits(&mut ui, 1).await;
    let before_commit = ui
        .fixture
        .repo
        .git(&["rev-parse", &format!("{}^", heads[0])]);

    let _ = ui
        .send(Message::UndoList(
            crate::features::undo_list::Message::Toggle,
        ))
        .await;
    let _ = ui
        .send(Message::UndoList(
            crate::features::undo_list::Message::Pick(1),
        ))
        .await;
    let _ = ui.click("[ Undo 2 ⏎ ]").await;

    assert_eq!(ui.fixture.repo.head(), before_commit, "the first step ran");
    let tab = &ui.app.tabs[0];
    assert_eq!(tab.undo.undo.len(), 1, "the failed step stays to undo");
    assert_eq!(tab.undo.redo.len(), 1);
    assert!(tab.undo_queue.is_none());
    assert!(
        ui.flash().contains("undo 2/2: create tag gone"),
        "{}",
        ui.flash()
    );
}
