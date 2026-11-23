//! Regression tests for what the history and the branch list do to the
//! repository: each test drives the real `App` (like `ui_sweep`), confirms
//! the dialog the way a user would and then checks the git result.
//!
//! They pin the behavior of a feature before it is changed: when a test
//! fails after a change, the feature no longer does what it used to.

use gf_core::model::Oid;
use gf_test_support::TestRepo;

use super::ui_sweep::Ui;
use super::*;
use crate::features::history::CommitAction;
use crate::features::sidebar::{self, Selection};

/// The fixture on a clean `main` (`first`, `second`, `third`), so that
/// operations are not blocked by its staged and modified files.
fn clean(repo: &TestRepo) {
    repo.git(&["reset", "-q", "--hard"]);
    repo.git(&["clean", "-fdq"]);
}

/// The fixture clean, on `feature` (at `second`) with one commit of its own.
fn feature_with_a_commit(repo: &TestRepo) {
    clean(repo);
    repo.git(&["switch", "-q", "feature"]);
    repo.write("feature.txt", "feature\n");
    repo.commit_all("feature work");
}

fn subjects(repo: &TestRepo, range: &str) -> Vec<String> {
    repo.git(&["log", "--format=%s", range])
        .lines()
        .map(str::to_owned)
        .collect()
}

fn oid(repo: &TestRepo, rev: &str) -> Oid {
    repo.git(&["rev-parse", rev]).parse().expect("object id")
}

/// Selects `oid` in the history and runs `action` on it.
async fn act_on(ui: &mut Ui, oid: &Oid, action: CommitAction) {
    let _ = ui
        .send(Message::History(history::Message::Select(oid.clone())))
        .await;
    let _ = ui
        .send(Message::History(history::Message::Act(action)))
        .await;
}

fn dialog_title(ui: &Ui) -> Option<&str> {
    ui.app.dialog.as_ref().map(|d| d.title.as_str())
}

async fn submit(ui: &mut Ui) {
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert!(ui.app.dialog.is_none(), "{:?}", ui.app.dialog);
}

async fn select_branch(ui: &mut Ui, name: &str) {
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Select(Selection::Ref(
            format!("refs/heads/{name}"),
        ))))
        .await;
}

#[tokio::test]
async fn cherry_pick_asks_first_then_adds_the_commit_on_the_current_branch() {
    let mut ui = Ui::open_with(feature_with_a_commit).await;
    let repo = &ui.fixture.repo;
    let third = oid(repo, "main");
    let before = repo.head();

    act_on(&mut ui, &third, CommitAction::CherryPick).await;
    assert_eq!(dialog_title(&ui), Some("Cherry-pick"));
    assert_eq!(
        ui.fixture.repo.head(),
        before,
        "nothing ran before the answer"
    );

    submit(&mut ui).await;
    let repo = &ui.fixture.repo;
    assert_eq!(subjects(repo, "-3"), ["third", "feature work", "second"]);
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), before);
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    assert_eq!(oid(repo, "main"), third, "main is untouched");
}

#[tokio::test]
async fn cherry_pick_cancelled_leaves_the_repository_alone() {
    let mut ui = Ui::open_with(feature_with_a_commit).await;
    let third = oid(&ui.fixture.repo, "main");
    let before = ui.fixture.repo.head();

    act_on(&mut ui, &third, CommitAction::CherryPick).await;
    let _ = ui.send(Message::Dialog(dialog::Message::Cancel)).await;

    assert!(ui.app.dialog.is_none());
    assert_eq!(ui.fixture.repo.head(), before);
}

#[tokio::test]
async fn revert_adds_a_commit_that_undoes_the_selected_one() {
    let mut ui = Ui::open_with(clean).await;
    let third = oid(&ui.fixture.repo, "HEAD");
    let before = ui.fixture.repo.head();

    act_on(&mut ui, &third, CommitAction::Revert).await;
    assert_eq!(dialog_title(&ui), Some("Revert"));
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_eq!(subjects(repo, "-1"), ["Revert \"third\""]);
    assert_eq!(
        repo.git(&["rev-parse", "HEAD~1"]),
        before,
        "history is kept"
    );
    assert_eq!(
        repo.git(&["show", "HEAD:src/lib.rs"]),
        "fn main() {}",
        "the file is back to its version before `third`"
    );
}

#[tokio::test]
async fn reset_moves_the_branch_and_keeps_the_changes_staged_by_default() {
    let mut ui = Ui::open_with(clean).await;
    let second = oid(&ui.fixture.repo, "HEAD~1");

    act_on(&mut ui, &second, CommitAction::Reset).await;
    assert_eq!(dialog_title(&ui), Some("Reset"), "reset always asks");
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_eq!(repo.head(), second.to_string());
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]),
        "src/lib.rs",
        "the dropped commit's changes stay staged (soft)"
    );
}

#[tokio::test]
async fn reset_can_be_undone_from_the_undo_list() {
    let mut ui = Ui::open_with(clean).await;
    let third = ui.fixture.repo.head();
    let second = oid(&ui.fixture.repo, "HEAD~1");

    act_on(&mut ui, &second, CommitAction::Reset).await;
    submit(&mut ui).await;
    assert_eq!(ui.fixture.repo.head(), second.to_string());
    assert_eq!(ui.app.tabs[0].undo.undo.len(), 1, "the reset is recorded");

    let _ = ui.send(Message::Undo).await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert_eq!(ui.fixture.repo.head(), third, "undo moves the branch back");
}

#[tokio::test]
async fn squash_into_previous_melds_two_commits_into_one() {
    let mut ui = Ui::open_with(clean).await;
    let third = oid(&ui.fixture.repo, "HEAD");
    let tree = ui.fixture.repo.git(&["rev-parse", "HEAD^{tree}"]);

    act_on(&mut ui, &third, CommitAction::SquashIntoPrevious).await;
    assert_eq!(dialog_title(&ui), Some("Squash 2 commits"));
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "2");
    assert_eq!(
        repo.git(&["rev-parse", "HEAD^{tree}"]),
        tree,
        "same content"
    );
    assert_eq!(subjects(repo, "-1").len(), 1);
    assert_eq!(repo.git(&["log", "-1", "--format=%s", "HEAD~1"]), "first");
}

#[tokio::test]
async fn merge_of_a_diverged_branch_makes_a_merge_commit() {
    let mut ui = Ui::open_with(|repo| {
        clean(repo);
        repo.git(&["switch", "-q", "feature"]);
        repo.write("feature.txt", "feature\n");
        repo.commit_all("feature work");
        repo.git(&["switch", "-q", "main"]);
    })
    .await;

    select_branch(&mut ui, "feature").await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Act(
            sidebar::Action::Merge,
        )))
        .await;
    assert_eq!(dialog_title(&ui), Some("Merge"));
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_eq!(
        repo.git(&["rev-list", "--parents", "-n1", "HEAD"])
            .split(' ')
            .count(),
        3,
        "a commit with two parents"
    );
    assert_eq!(repo.git(&["show", "HEAD:feature.txt"]), "feature");
    assert_eq!(repo.git(&["show", "HEAD:src/lib.rs"]).lines().count(), 3);
}

#[tokio::test]
async fn merge_fast_forwards_when_it_can() {
    let mut ui = Ui::open_with(|repo| {
        clean(repo);
        repo.git(&["switch", "-q", "feature"]);
    })
    .await;
    let main = ui.fixture.repo.git(&["rev-parse", "main"]);

    select_branch(&mut ui, "main").await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Act(
            sidebar::Action::Merge,
        )))
        .await;
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_eq!(
        repo.head(),
        main,
        "feature moved to main without a new commit"
    );
    assert_eq!(repo.git(&["rev-parse", "main"]), main);
}

#[tokio::test]
async fn rebase_onto_replays_the_branch_on_top_of_the_selected_one() {
    let mut ui = Ui::open_with(feature_with_a_commit).await;
    let main = ui.fixture.repo.git(&["rev-parse", "main"]);
    let old = ui.fixture.repo.head();

    select_branch(&mut ui, "main").await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Act(
            sidebar::Action::RebaseOnto,
        )))
        .await;
    assert_eq!(dialog_title(&ui), Some("Rebase"));
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), main);
    assert_eq!(subjects(repo, "-1"), ["feature work"]);
    assert_ne!(repo.head(), old, "the commit was rewritten");
    assert_eq!(repo.git(&["branch", "--show-current"]), "feature");
}

async fn sidebar_act(ui: &mut Ui, selection: Selection, action: sidebar::Action) {
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Select(selection)))
        .await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Act(action)))
        .await;
}

fn tag_ref(name: &str) -> Selection {
    Selection::Ref(format!("refs/tags/{name}"))
}

async fn field(ui: &mut Ui, i: usize, text: &str) {
    let _ = ui
        .send(Message::Dialog(dialog::Message::Field(i, text.into())))
        .await;
}

#[tokio::test]
async fn a_new_tag_without_a_message_is_lightweight() {
    let mut ui = Ui::open_with(clean).await;

    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Act(
            sidebar::Action::NewTag,
        )))
        .await;
    assert_eq!(dialog_title(&ui), Some("New tag"));
    field(&mut ui, 0, "v2").await;
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_eq!(repo.git(&["cat-file", "-t", "v2"]), "commit");
    assert_eq!(repo.git(&["rev-parse", "v2"]), repo.head(), "tags HEAD");
}

#[tokio::test]
async fn a_new_tag_with_a_message_is_annotated() {
    let mut ui = Ui::open_with(clean).await;
    let second = oid(&ui.fixture.repo, "HEAD~1");

    let _ = ui
        .send(Message::History(history::Message::Select(second.clone())))
        .await;
    let _ = ui.send(Message::History(history::Message::TagHere)).await;
    assert_eq!(dialog_title(&ui), Some("New tag"));
    field(&mut ui, 0, "v1.5").await;
    field(&mut ui, 1, "half way").await;
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_eq!(repo.git(&["cat-file", "-t", "v1.5"]), "tag");
    assert_eq!(
        repo.git(&["rev-parse", "v1.5^{commit}"]),
        second.to_string()
    );
    assert_eq!(
        repo.git(&["tag", "-l", "--format=%(contents)", "v1.5"]),
        "half way"
    );
}

#[tokio::test]
async fn deleting_a_tag_asks_first_and_undo_brings_it_back() {
    let mut ui = Ui::open_with(clean).await;
    let tagged = ui.fixture.repo.git(&["rev-parse", "v1"]);

    sidebar_act(&mut ui, tag_ref("v1"), sidebar::Action::Delete).await;
    assert_eq!(dialog_title(&ui), Some("Delete tag"));
    assert_eq!(
        ui.fixture.repo.git(&["tag", "-l"]),
        "v1",
        "not before the answer"
    );
    submit(&mut ui).await;
    assert_eq!(ui.fixture.repo.git(&["tag", "-l"]), "");

    let _ = ui.send(Message::Undo).await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert_eq!(ui.fixture.repo.git(&["rev-parse", "v1"]), tagged);
}

#[tokio::test]
async fn renaming_a_branch_moves_it_and_keeps_its_commits() {
    let mut ui = Ui::open_with(clean).await;
    let tip = ui.fixture.repo.git(&["rev-parse", "feature"]);

    sidebar_act(
        &mut ui,
        Selection::Ref("refs/heads/feature".into()),
        sidebar::Action::Rename,
    )
    .await;
    assert_eq!(dialog_title(&ui), Some("Rename branch"));
    field(&mut ui, 0, "renamed").await;
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    let branches = repo.git(&["branch", "--format=%(refname:short)"]);
    assert!(branches.lines().any(|b| b == "renamed"), "{branches}");
    assert!(!branches.lines().any(|b| b == "feature"), "{branches}");
    assert_eq!(repo.git(&["rev-parse", "renamed"]), tip);
}

#[tokio::test]
async fn stash_pop_applies_the_stash_and_removes_it() {
    let mut ui = Ui::open_with(clean).await;

    sidebar_act(&mut ui, Selection::Stash(0), sidebar::Action::StashPop).await;
    assert_eq!(dialog_title(&ui), Some("Pop stash"));
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_eq!(repo.git(&["stash", "list"]), "", "the stash is gone");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).expect("a.txt"),
        "stashed\n"
    );
}

#[tokio::test]
async fn stash_apply_keeps_the_stash_in_the_list() {
    let mut ui = Ui::open_with(clean).await;

    sidebar_act(&mut ui, Selection::Stash(0), sidebar::Action::StashApply).await;
    assert_eq!(dialog_title(&ui), Some("Apply stash"));
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_eq!(repo.git(&["stash", "list"]).lines().count(), 1);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).expect("a.txt"),
        "stashed\n"
    );
}

#[tokio::test]
async fn dropping_a_stash_asks_first_and_removes_only_that_stash() {
    let mut ui = Ui::open_with(|repo| {
        clean(repo);
        repo.write("second.txt", "more\n");
        repo.git(&["add", "second.txt"]);
        repo.git(&["stash", "push", "-q", "-m", "newer"]);
    })
    .await;
    assert_eq!(ui.fixture.repo.git(&["stash", "list"]).lines().count(), 2);

    sidebar_act(&mut ui, Selection::Stash(0), sidebar::Action::Delete).await;
    assert_eq!(dialog_title(&ui), Some("Drop stash"));
    submit(&mut ui).await;

    let left = ui.fixture.repo.git(&["stash", "list"]);
    assert_eq!(left.lines().count(), 1);
    assert!(left.contains("wip"), "the older stash stays: {left}");
}

#[tokio::test]
async fn stashing_puts_every_change_aside_including_untracked_files() {
    let mut ui = Ui::open().await;
    assert_ne!(ui.fixture.repo.git(&["status", "--porcelain"]), "");

    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Act(
            sidebar::Action::NewStash,
        )))
        .await;
    assert_eq!(dialog_title(&ui), Some("Stash changes"));
    field(&mut ui, 0, "parked").await;
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_eq!(repo.git(&["status", "--porcelain"]), "", "a clean tree");
    assert!(!repo.path().join("c.txt").exists(), "untracked files too");
    assert!(repo.git(&["stash", "list"]).contains("parked"));
}

#[tokio::test]
async fn discarding_a_modified_file_restores_it_and_leaves_the_rest() {
    let mut ui = Ui::open().await;

    let _ = ui
        .send(Message::Changes(changes::Message::Discard(
            "a.txt".into(),
            changes::Group::Unstaged,
        )))
        .await;
    assert_eq!(dialog_title(&ui), Some("Discard changes"));
    let before = std::fs::read_to_string(ui.fixture.repo.path().join("a.txt")).expect("a.txt");
    assert!(before.contains("changed"), "not before the answer");
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    let after = std::fs::read_to_string(repo.path().join("a.txt")).expect("a.txt");
    assert_eq!(after, "one\ntwo\nthree\nfour\n");
    assert!(
        repo.path().join("c.txt").exists(),
        "other files are untouched"
    );
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "b.txt");
}

#[tokio::test]
async fn discarding_an_untracked_file_deletes_it_and_undo_restores_it() {
    let mut ui = Ui::open().await;

    let _ = ui
        .send(Message::Changes(changes::Message::Discard(
            "c.txt".into(),
            changes::Group::Untracked,
        )))
        .await;
    submit(&mut ui).await;
    assert!(!ui.fixture.repo.path().join("c.txt").exists());

    let _ = ui.send(Message::Undo).await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert_eq!(
        std::fs::read_to_string(ui.fixture.repo.path().join("c.txt")).expect("restored"),
        "untracked\n"
    );
}

#[tokio::test]
async fn amend_adds_the_staged_changes_to_the_last_commit_and_keeps_its_message() {
    let mut ui = Ui::open_with(|repo| {
        clean(repo);
        repo.write("late.txt", "forgotten\n");
        repo.git(&["add", "late.txt"]);
    })
    .await;
    let before = ui.fixture.repo.head();
    let parent = ui.fixture.repo.git(&["rev-parse", "HEAD~1"]);

    let _ = ui
        .send(Message::Commit(commit_box::Message::Submit { amend: true }))
        .await;
    assert_eq!(dialog_title(&ui), Some("Amend last commit"));
    assert_eq!(ui.fixture.repo.head(), before, "not before the answer");
    submit(&mut ui).await;

    let repo = &ui.fixture.repo;
    assert_ne!(repo.head(), before, "the commit was replaced");
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]), parent);
    assert_eq!(subjects(repo, "-1"), ["third"]);
    assert_eq!(repo.git(&["show", "HEAD:late.txt"]), "forgotten");
}

#[tokio::test]
async fn locking_a_worktree_is_confirmed_and_unlocking_reverts_it() {
    let mut ui = Ui::open_with(clean).await;
    let folder = tempfile::tempdir().expect("worktree parent");
    let path = folder
        .path()
        .canonicalize()
        .map(gf_test_support::without_verbatim_prefix)
        .expect("worktree parent path")
        .join("wt");
    ui.fixture.repo.git(&[
        "worktree",
        "add",
        "-q",
        path.to_str().expect("path"),
        "feature",
    ]);
    let _ = ui.send(Message::Toolbar(toolbar::Message::Refresh)).await;
    let _ = ui
        .send(Message::RefsMode(worktrees::RefsMode::Worktrees))
        .await;
    let locked = |ui: &Ui| {
        ui.fixture
            .repo
            .git(&["worktree", "list", "--porcelain"])
            .contains("locked")
    };

    let _ = ui
        .send(Message::Worktrees(worktrees::Message::Select(path.clone())))
        .await;
    let _ = ui
        .send(Message::Worktrees(worktrees::Message::Act(
            worktrees::Action::Lock,
        )))
        .await;
    assert_eq!(dialog_title(&ui), Some("Lock worktree"));
    assert!(!locked(&ui), "not before the answer");
    submit(&mut ui).await;
    assert!(locked(&ui));

    let _ = ui
        .send(Message::Worktrees(worktrees::Message::Act(
            worktrees::Action::Unlock,
        )))
        .await;
    assert_eq!(dialog_title(&ui), Some("Unlock worktree"));
    submit(&mut ui).await;
    assert!(!locked(&ui));
}

#[tokio::test]
async fn pruning_forgets_a_worktree_whose_folder_is_gone_but_keeps_its_branch() {
    let mut ui = Ui::open_with(clean).await;
    let folder = tempfile::tempdir().expect("worktree parent");
    let path = folder
        .path()
        .canonicalize()
        .map(gf_test_support::without_verbatim_prefix)
        .expect("worktree parent path")
        .join("gone");
    ui.fixture.repo.git(&[
        "worktree",
        "add",
        "-q",
        path.to_str().expect("path"),
        "feature",
    ]);
    std::fs::remove_dir_all(&path).expect("remove the folder");
    let trees = |ui: &Ui| {
        ui.fixture
            .repo
            .git(&["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count()
    };
    assert_eq!(trees(&ui), 2, "git still lists it");

    let _ = ui.send(Message::Worktrees(worktrees::Message::Prune)).await;
    assert_eq!(dialog_title(&ui), Some("Prune worktrees"));
    submit(&mut ui).await;

    assert_eq!(trees(&ui), 1);
    assert!(
        ui.fixture
            .repo
            .git(&["branch", "--list", "feature"])
            .contains("feature")
    );
}
