#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;

use common::open;
use gf_git::conflict::Choice;
use gf_git::{Integration, LogQuery, MergeStyle, RepoState, Side, TodoAction, Verdict};
use gf_test_support::TestRepo;

/// main and topic both change `f.txt` line 1 differently.
fn diverged() -> TestRepo {
    let repo = TestRepo::new();
    repo.write("f.txt", "base\nshared\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "topic"]);
    repo.write("f.txt", "topic\nshared\n");
    repo.commit_all("topic change");
    repo.git(&["switch", "-q", "main"]);
    repo.write("f.txt", "main\nshared\n");
    repo.commit_all("main change");
    repo
}

fn summaries(repo: &TestRepo) -> Vec<String> {
    repo.git(&["log", "--format=%s"])
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn merge_conflict_resolve_and_continue() {
    let repo = diverged();
    let r = open(&repo).await;

    assert_eq!(
        r.merge("topic", MergeStyle::Default).await.unwrap(),
        Integration::Conflicts(1)
    );
    assert!(
        matches!(r.state().await.unwrap(), RepoState::Merging { message: Some(m) } if m.contains("topic"))
    );

    let segs = r
        .conflict_segments(Path::new("f.txt"))
        .await
        .unwrap()
        .unwrap();
    let left = r
        .resolve_blocks(Path::new("f.txt"), &segs, &[Some(Choice::Theirs)])
        .await
        .unwrap();
    assert_eq!(left, 0);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("f.txt")).unwrap(),
        "topic\nshared\n"
    );

    assert_eq!(r.continue_operation().await.unwrap(), Integration::Done);
    assert!(r.state().await.unwrap().is_clean());
    assert_eq!(
        repo.git(&["rev-list", "--parents", "-n1", "HEAD"])
            .split(' ')
            .count(),
        3,
        "merge commit"
    );
}

#[tokio::test]
async fn merge_abort_and_fast_forward() {
    let repo = diverged();
    let r = open(&repo).await;
    let before = repo.head();
    r.merge("topic", MergeStyle::Default).await.unwrap();
    r.abort_operation().await.unwrap();
    assert!(r.state().await.unwrap().is_clean());
    assert_eq!(repo.head(), before);

    // A branch ahead of main merges as a fast-forward; --no-ff forces a commit.
    repo.git(&["switch", "-q", "-c", "ahead"]);
    repo.write("g.txt", "g\n");
    let ahead = repo.commit_all("ahead");
    repo.git(&["switch", "-q", "main"]);
    assert!(r.merge("ahead", MergeStyle::FastForwardOnly).await.is_ok());
    assert_eq!(repo.head(), ahead);
}

#[tokio::test]
async fn rebase_with_conflict_take_side_and_continue() {
    let repo = diverged();
    repo.git(&["switch", "-q", "topic"]);
    let r = open(&repo).await;
    assert_eq!(r.rebase("main").await.unwrap(), Integration::Conflicts(1));
    assert!(matches!(
        r.state().await.unwrap(),
        RepoState::Rebasing { .. }
    ));
    // During a rebase "theirs" is the commit being replayed (topic's).
    r.take_side(Path::new("f.txt"), Side::Theirs).await.unwrap();
    assert_eq!(r.continue_operation().await.unwrap(), Integration::Done);
    assert!(r.state().await.unwrap().is_clean());
    assert_eq!(
        summaries(&repo),
        ["topic change", "main change", "base"],
        "linear history"
    );
}

#[tokio::test]
async fn cherry_pick_and_revert() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("a");
    repo.git(&["switch", "-q", "-c", "topic"]);
    repo.write("b.txt", "b\n");
    let b = repo.commit_all("add b");
    repo.git(&["switch", "-q", "main"]);
    let r = open(&repo).await;

    assert_eq!(r.cherry_pick(&[b]).await.unwrap(), Integration::Done);
    assert!(repo.path().join("b.txt").exists());
    let picked = repo.head();
    assert_eq!(r.revert(&picked).await.unwrap(), Integration::Done);
    assert!(!repo.path().join("b.txt").exists());
    assert!(summaries(&repo)[0].starts_with("Revert"));
}

#[tokio::test]
async fn interactive_rebase_reword_squash_drop() {
    let repo = TestRepo::new();
    repo.write("f.txt", "0\n");
    let base = repo.commit_all("base");
    for (i, msg) in ["one", "two", "three", "four"].iter().enumerate() {
        repo.write(format!("{i}.txt"), msg);
        repo.commit_all(msg);
    }
    let r = open(&repo).await;
    let base = base.parse().unwrap();
    let mut plan = r.rebase_plan(Some(&base)).await.unwrap();
    assert_eq!(
        plan.iter()
            .map(|i| i.commit.summary.as_str())
            .collect::<Vec<_>>(),
        ["one", "two", "three", "four"]
    );

    plan[0].action = TodoAction::Reword("ONE (reworded)".into());
    plan[2].action = TodoAction::Squash("two + three".into());
    plan[3].action = TodoAction::Drop;
    assert_eq!(
        r.rebase_interactive(Some(&base), &plan).await.unwrap(),
        Integration::Done
    );
    assert_eq!(summaries(&repo), ["two + three", "ONE (reworded)", "base"]);
    assert!(repo.path().join("2.txt").exists(), "squashed content kept");
    assert!(
        !repo.path().join("3.txt").exists(),
        "dropped commit removed"
    );
}

#[tokio::test]
async fn squash_range_with_autostash_and_pushed_count() {
    let repo = TestRepo::new();
    repo.write("f.txt", "0\n");
    repo.commit_all("base");
    let mut ids = Vec::new();
    for (i, msg) in ["one", "two", "three", "four"].iter().enumerate() {
        repo.write(format!("{i}.txt"), msg);
        ids.push(repo.commit_all(msg));
    }
    // "one" is on a remote already.
    repo.git(&["update-ref", "refs/remotes/origin/main", &ids[0]]);
    // Uncommitted change: needs the autostash.
    repo.write("f.txt", "dirty\n");
    let r = open(&repo).await;
    let pick = |i: &[usize]| -> Vec<gf_core::model::Oid> {
        i.iter().map(|&i| ids[i].parse().unwrap()).collect()
    };

    let preview = r.squash_preview(&pick(&[2, 0, 1])).await.unwrap();
    assert_eq!(
        preview
            .commits
            .iter()
            .map(|c| c.summary.as_str())
            .collect::<Vec<_>>(),
        ["one", "two", "three"]
    );
    assert_eq!(preview.pushed, 1);
    assert!(preview.base.is_some());
    assert!(r.squash_preview(&pick(&[0, 2])).await.is_err(), "gap");

    assert_eq!(
        r.squash(&pick(&[1, 2]), "two + three", true).await.unwrap(),
        Integration::Done
    );
    assert_eq!(summaries(&repo), ["four", "two + three", "one", "base"]);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("f.txt")).unwrap(),
        "dirty\n",
        "uncommitted change restored"
    );
}

#[tokio::test]
async fn squash_rejects_a_merge_in_the_rewrite_plan_before_changing_history() {
    let repo = TestRepo::new();
    repo.write("base.txt", "base");
    repo.commit_all("base");
    repo.git(&["branch", "topic"]);
    repo.write("first.txt", "first");
    let first = repo.commit_all("first");
    repo.write("second.txt", "second");
    let second = repo.commit_all("second");
    repo.git(&["switch", "topic"]);
    repo.write("topic.txt", "topic");
    repo.commit_all("topic");
    repo.git(&["switch", "main"]);
    repo.git(&["merge", "--no-ff", "--no-edit", "topic"]);

    let r = open(&repo).await;
    let head = repo.git(&["rev-parse", "HEAD"]);
    let ids = [second.parse().unwrap(), first.parse().unwrap()];
    let preview_error = r.squash_preview(&ids).await.unwrap_err().to_string();
    assert!(preview_error.contains("includes merge commit"));
    assert!(preview_error.contains(&head[..7]));
    assert_eq!(
        r.squash(&ids, "combined", false)
            .await
            .unwrap_err()
            .to_string(),
        preview_error
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(r.state().await.unwrap(), RepoState::Clean);
    assert!(!repo.path().join(".git/gf-rebase").exists());
}

#[tokio::test]
async fn squash_from_the_root_commit() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    let root = repo.commit_all("root");
    repo.write("b.txt", "b\n");
    let second = repo.commit_all("second");
    let r = open(&repo).await;
    let ids = [second.parse().unwrap(), root.parse().unwrap()];
    assert_eq!(r.squash_preview(&ids).await.unwrap().base, None);
    assert_eq!(
        r.squash(&ids, "everything", false).await.unwrap(),
        Integration::Done
    );
    assert_eq!(summaries(&repo), ["everything"]);
    assert!(repo.path().join("b.txt").exists());
}

#[tokio::test]
async fn reflog_and_undo() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    let first = repo.commit_all("first");
    repo.write("a.txt", "b\n");
    repo.commit_all("second");
    let r = open(&repo).await;
    let log = r.reflog(5).await.unwrap();
    assert!(log[0].message.contains("second"));
    assert_eq!(
        r.head_oid().await.unwrap().unwrap().to_string(),
        repo.head()
    );

    // Soft undo of a commit keeps its changes staged.
    r.undo_to(&first.parse().unwrap(), true).await.unwrap();
    assert_eq!(repo.head(), first);
    assert!(repo.git(&["diff", "--cached"]).contains("+b"));
    // Keep-mode undo restores files.
    repo.git(&["commit", "-q", "-m", "again"]);
    r.undo_to(&first.parse().unwrap(), false).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a\n"
    );
}

#[tokio::test]
async fn blame_and_file_history() {
    let repo = TestRepo::new();
    repo.write("f.txt", "one\ntwo\n");
    repo.commit_all("first");
    repo.write("f.txt", "one\nTWO\n");
    repo.commit_all("second");
    repo.git(&["mv", "f.txt", "g.txt"]);
    repo.commit_all("rename");
    let r = open(&repo).await;

    let blame = r.blame(Path::new("g.txt"), None).await.unwrap();
    assert_eq!(blame.len(), 2);
    assert_eq!(blame[0].summary, "first");
    assert_eq!(blame[1].summary, "second");
    assert_eq!(blame[1].content, "TWO");

    let history = r
        .log(&LogQuery {
            paths: vec!["g.txt".into()],
            follow: true,
            ..LogQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(history.len(), 3, "--follow crosses the rename");
}

#[tokio::test]
async fn worktrees() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("a");
    let r = open(&repo).await;
    let extra = tempfile::tempdir().unwrap();
    let path = extra.path().join("wt");
    r.add_worktree(&path, "side", true).await.unwrap();
    let list = r.worktrees().await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().any(|w| w.branch.as_deref() == Some("side")));
    r.remove_worktree(&path, false).await.unwrap();
    assert_eq!(r.worktrees().await.unwrap().len(), 1);
}

#[tokio::test]
async fn submodules() {
    let sub = TestRepo::new();
    sub.write("lib.txt", "lib\n");
    sub.commit_all("lib");
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("a");
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        sub.path().to_str().unwrap(),
        "libs/sub",
    ]);
    repo.commit_all("add submodule");
    let r = open(&repo).await;
    let list = r.submodules().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].path, Path::new("libs/sub"));
    assert_eq!(list[0].status, gf_git::SubmoduleStatus::UpToDate);
}

#[tokio::test]
async fn bisect_finds_the_first_bad_commit() {
    let repo = TestRepo::new();
    let mut ids = Vec::new();
    for i in 0..8 {
        repo.write("f.txt", if i >= 5 { "bug\n" } else { "ok\n" });
        repo.write(format!("{i}.txt"), "x");
        ids.push(repo.commit_all(&format!("c{i}")));
    }
    let r = open(&repo).await;
    let mut p = r.bisect_start(&ids[7], &ids[0]).await.unwrap();
    assert_eq!(r.state().await.unwrap(), RepoState::Bisecting);
    for _ in 0..10 {
        if p.first_bad.is_some() {
            break;
        }
        let bad = std::fs::read_to_string(repo.path().join("f.txt")).unwrap() == "bug\n";
        p = r
            .bisect_mark(if bad { Verdict::Bad } else { Verdict::Good })
            .await
            .unwrap();
    }
    assert_eq!(p.first_bad.map(|o| o.to_string()), Some(ids[5].clone()));
    r.bisect_reset().await.unwrap();
    assert!(r.state().await.unwrap().is_clean());
}

#[tokio::test]
async fn revert_and_cherry_pick_several() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("a");
    repo.git(&["switch", "-q", "-c", "topic"]);
    repo.write("b.txt", "b\n");
    let b = repo.commit_all("add b");
    repo.write("c.txt", "c\n");
    let c = repo.commit_all("add c");
    repo.git(&["switch", "-q", "main"]);
    let r = open(&repo).await;

    // Oldest first for cherry-pick.
    assert_eq!(r.cherry_pick(&[b, c]).await.unwrap(), Integration::Done);
    assert!(repo.path().join("b.txt").exists() && repo.path().join("c.txt").exists());
    // Newest first for revert.
    let picked: Vec<String> = repo
        .git(&["rev-list", "-n2", "HEAD"])
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(r.revert_many(&picked).await.unwrap(), Integration::Done);
    assert!(!repo.path().join("b.txt").exists() && !repo.path().join("c.txt").exists());
}

#[tokio::test]
async fn reset_modes_treat_changes_differently() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    let base = repo.commit_all("base");
    repo.write("a.txt", "two\n");
    repo.commit_all("second");
    let r = open(&repo).await;
    let base: gf_core::model::Oid = base.parse().expect("oid");

    // Soft: the second commit's change stays staged.
    let top = repo.head();
    r.reset(&base, gf_git::ResetMode::Soft).await.expect("soft");
    assert_eq!(repo.head(), base.as_str());
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "a.txt");

    // Mixed: unstaged, still in the file.
    repo.git(&["reset", "--hard", &top]);
    r.reset(&base, gf_git::ResetMode::Mixed)
        .await
        .expect("mixed");
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "");
    assert_eq!(repo.git(&["diff", "--name-only"]), "a.txt");

    // Hard: the file is back to the target.
    r.reset(&base, gf_git::ResetMode::Hard).await.expect("hard");
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).expect("read"),
        "one\n"
    );
}
