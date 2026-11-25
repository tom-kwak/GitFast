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
    r.add_worktree(&path, "side", true, None).await.unwrap();
    let list = r.worktrees().await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().any(|w| w.branch.as_deref() == Some("side")));

    // Asked from the linked tree: the main tree still comes first, once.
    let linked = gf_git::Repository::open(&path, common::runner(&repo))
        .await
        .unwrap();
    let from_linked = linked.worktrees().await.unwrap();
    let branches: Vec<_> = from_linked.iter().map(|w| w.branch.clone()).collect();
    assert_eq!(
        branches,
        list.iter().map(|w| w.branch.clone()).collect::<Vec<_>>()
    );
    assert_ne!(from_linked[0].branch.as_deref(), Some("side"));
    assert!(linked.is_linked_worktree());
    assert!(!r.is_linked_worktree());

    r.remove_worktree(&path, false).await.unwrap();
    assert_eq!(r.worktrees().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_new_worktree_gets_the_ignored_files_worktreeinclude_lists() {
    let repo = TestRepo::new();
    repo.write(".gitignore", ".env\nsecret/\nbuild/\n");
    repo.write(
        ".worktreeinclude",
        "# local files\n.env\nsecret/**\n!secret/skip.txt\nREADME.md\nnotes.txt\n",
    );
    repo.write("README.md", "hi\n");
    repo.commit_all("a");
    repo.write(".env", "A=1\n");
    repo.write("secret/keys/a.pem", "key\n");
    repo.write("secret/skip.txt", "no\n");
    repo.write("build/out.bin", "ignored, not listed\n");
    repo.write("notes.txt", "listed, not ignored\n");
    let r = open(&repo).await;
    let extra = tempfile::tempdir().unwrap();
    let path = extra.path().join("wt");
    r.add_worktree(&path, "side", true, None).await.unwrap();

    let copy = r.copy_included(&path).await.unwrap();
    assert!(copy.listed);
    let mut copied = copy.copied.clone();
    copied.sort();
    assert_eq!(
        copied,
        vec![
            std::path::PathBuf::from(".env"),
            std::path::PathBuf::from("secret/keys/a.pem")
        ]
    );
    assert!(copy.failed.is_empty());
    assert_eq!(std::fs::read_to_string(path.join(".env")).unwrap(), "A=1\n");
    for absent in ["secret/skip.txt", "build/out.bin", "notes.txt"] {
        assert!(!path.join(absent).exists(), "{absent} is not copied");
    }

    // Again: what is there stays.
    let again = r.copy_included(&path).await.unwrap();
    assert!(again.copied.is_empty());
    assert_eq!(again.kept.len(), 2);

    // Without the file, nothing is read or copied.
    let bare = TestRepo::new();
    bare.write(".gitignore", ".env\n");
    bare.commit_all("a");
    bare.write(".env", "A=1\n");
    let b = open(&bare).await;
    let other = extra.path().join("wt2");
    b.add_worktree(&other, "side", true, None).await.unwrap();
    assert_eq!(
        b.copy_included(&other).await.unwrap(),
        gf_git::IncludedCopy::default()
    );
    assert!(!other.join(".env").exists());
}

#[tokio::test]
async fn work_base_is_only_the_one_known_for_sure() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("a");
    repo.git(&["branch", "develop"]);
    // No message: nothing names where it came from.
    repo.git(&["update-ref", "refs/heads/loose", "HEAD"]);
    let r = open(&repo).await;
    let extra = tempfile::tempdir().unwrap();
    let made = extra.path().join("made");
    r.add_worktree(&made, "feature/x", true, Some("develop"))
        .await
        .unwrap();
    let linked = gf_git::Repository::open(&made, common::runner(&repo))
        .await
        .unwrap();
    assert_eq!(
        linked.work_base().await.unwrap().as_deref(),
        Some("develop")
    );
    // Nothing records where `loose` came from: a guess is not enough.
    let guessed = extra.path().join("loose");
    r.add_worktree(&guessed, "loose", false, None)
        .await
        .unwrap();
    let linked = gf_git::Repository::open(&guessed, common::runner(&repo))
        .await
        .unwrap();
    assert_eq!(linked.work_base().await.unwrap(), None);
    // On the default base itself: nothing to go back into.
    assert_eq!(r.work_base().await.unwrap(), None);
}

#[tokio::test]
async fn worktree_status_reports_changes_work_and_merge() {
    let repo = TestRepo::new();
    repo.git(&["branch", "-M", "main"]);
    repo.write("a.txt", "a\n");
    repo.commit_all("a");
    let r = open(&repo).await;
    assert_eq!(r.default_base().await.unwrap().as_deref(), Some("main"));
    let extra = tempfile::tempdir().unwrap();
    let path = extra.path().join("wt");
    r.add_worktree(&path, "agent", true, Some("main"))
        .await
        .unwrap();
    let git = |args: &[&str]| {
        let out = repo
            .command()
            .current_dir(&path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    std::fs::write(path.join("b.txt"), "b\n").unwrap();
    git(&["add", "b.txt"]);
    git(&["commit", "-qm", "add b"]);
    std::fs::write(path.join("a.txt"), "changed\n").unwrap();
    std::fs::write(path.join("new.txt"), "n\n").unwrap();

    let s = r.worktree_status(&path, Some("main")).await.unwrap();
    assert_eq!((s.staged, s.unstaged, s.untracked), (0, 1, 1));
    assert_eq!((s.ahead_of_base, s.behind_base), (1, 0));
    assert_eq!(
        s.merge_base.as_ref().map(ToString::to_string),
        Some(repo.head())
    );
    assert_eq!(
        s.last_commit.as_ref().map(|c| c.subject.as_str()),
        Some("add b")
    );
    assert!(s.last_activity.is_some());
    assert!(!s.merged());

    // A fresh tree the base moved past is not "merged".
    let fresh = extra.path().join("fresh");
    r.add_worktree(&fresh, "fresh", true, Some("main"))
        .await
        .unwrap();
    repo.write("c.txt", "c\n");
    repo.commit_all("main moves on");
    let s = r.worktree_status(&fresh, Some("main")).await.unwrap();
    assert!(s.behind_base == 1 && !s.merged(), "{s:?}");

    // The main tree is on the base: no distance to itself.
    let main = r.worktree_status(repo.path(), Some("main")).await.unwrap();
    assert!(main.base.is_none() && main.is_clean());

    // Merge the work, move main on: the agent's tree is merged.
    git(&["checkout", "--", "a.txt"]);
    std::fs::remove_file(path.join("new.txt")).unwrap();
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge agent", "agent"]);
    let s = r.worktree_status(&path, Some("main")).await.unwrap();
    assert!(s.is_clean() && s.merged(), "{s:?}");

    r.lock_worktree(&path, true).await.unwrap();
    assert!(r.worktrees().await.unwrap().iter().any(|w| w.locked));
    r.lock_worktree(&path, false).await.unwrap();
    assert!(!r.worktrees().await.unwrap().iter().any(|w| w.locked));
}

#[tokio::test]
async fn a_trees_base_is_the_branch_it_was_made_from() {
    let repo = TestRepo::new();
    repo.git(&["branch", "-M", "main"]);
    repo.write("a.txt", "a\n");
    repo.commit_all("a");
    repo.git(&["switch", "-q", "-c", "develop"]);
    repo.write("d.txt", "d\n");
    repo.commit_all("develop work");
    let r = open(&repo).await;
    let extra = tempfile::tempdir().unwrap();
    let base_of = |path: std::path::PathBuf| {
        let r = &r;
        async move {
            let s = r.worktree_status(&path, Some("main")).await.unwrap();
            (s.base, s.base_guessed)
        }
    };

    // Made by gf from HEAD (develop): recorded, not guessed.
    let made = extra.path().join("made");
    r.add_worktree(&made, "made", true, None).await.unwrap();
    assert_eq!(
        repo.git(&["config", "--get", "branch.made.gf-base"]).trim(),
        "develop"
    );
    assert_eq!(base_of(made.clone()).await, (Some("develop".into()), false));

    // Named in its creation (`git branch x develop`): read from the reflog.
    repo.git(&["branch", "named", "develop"]);
    let named = extra.path().join("named");
    r.add_worktree(&named, "named", false, None).await.unwrap();
    assert_eq!(base_of(named).await, (Some("develop".into()), false));

    // Made from HEAD outside gf: the nearest usual branch, a guess.
    repo.git(&["switch", "-q", "-c", "loose"]);
    repo.git(&["switch", "-q", "develop"]);
    let loose = extra.path().join("loose");
    r.add_worktree(&loose, "loose", false, None).await.unwrap();
    assert_eq!(base_of(loose.clone()).await, (Some("develop".into()), true));

    // The user's choice wins; forgetting it goes back to guessing.
    r.set_branch_base("loose", Some("main")).await.unwrap();
    assert_eq!(base_of(loose.clone()).await, (Some("main".into()), false));
    r.set_branch_base("loose", None).await.unwrap();
    r.set_branch_base("loose", None).await.unwrap();
    assert_eq!(base_of(loose).await, (Some("develop".into()), true));

    // The default base itself has none.
    repo.git(&["switch", "-q", "main"]);
    let main = r.worktree_status(repo.path(), Some("main")).await.unwrap();
    assert_eq!(main.base, None);
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

/// What a tree's card says of where its work stands, as the worktrees view
/// decides it from a clean tree's status.
fn standing(s: &gf_git::WorktreeStatus) -> &'static str {
    if s.merged() {
        "merged"
    } else if s.ahead_of_base > 0 {
        "ready"
    } else if s.behind_base > 0 {
        "behind"
    } else {
        "clean"
    }
}

/// A tree on `topic` made from main, `act` run on (main repo, tree), then
/// how it stands against main.
async fn standing_after(act: impl FnOnce(&TestRepo, &Path)) -> (&'static str, u32) {
    let repo = TestRepo::new();
    repo.git(&["branch", "-M", "main"]);
    repo.write("a.txt", "a\n");
    repo.commit_all("a");
    let r = open(&repo).await;
    let extra = tempfile::tempdir().unwrap();
    let wt = extra.path().join("wt");
    r.add_worktree(&wt, "topic", true, Some("main"))
        .await
        .unwrap();
    act(&repo, &wt);
    let s = r.worktree_status(&wt, Some("main")).await.unwrap();
    assert!(s.is_clean(), "{s:?}");
    (standing(&s), s.ahead_of_base)
}

fn in_tree(repo: &TestRepo, wt: &Path, args: &[&str]) {
    let out = repo.command().current_dir(wt).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn main_moves(repo: &TestRepo, file: &str) {
    repo.write(file, "m\n");
    repo.commit_all(file);
}

fn tree_commits(repo: &TestRepo, wt: &Path, file: &str) {
    std::fs::write(wt.join(file), "w\n").unwrap();
    in_tree(repo, wt, &["add", file]);
    in_tree(repo, wt, &["commit", "-qm", file]);
}

#[tokio::test]
async fn taking_in_the_base_is_not_work_that_got_merged() {
    // Brought up to date without a commit of its own: not merged.
    let ff = standing_after(|r, wt| {
        main_moves(r, "m1");
        in_tree(r, wt, &["merge", "-q", "main"]);
    });
    assert_eq!(ff.await, ("clean", 0), "fast-forward update");
    let rebased = standing_after(|r, wt| {
        main_moves(r, "m1");
        in_tree(r, wt, &["rebase", "-q", "main"]);
    });
    assert_eq!(rebased.await, ("clean", 0), "rebase update");
    let reset = standing_after(|r, wt| {
        main_moves(r, "m1");
        in_tree(r, wt, &["reset", "-q", "--hard", "main"]);
    });
    assert_eq!(reset.await, ("clean", 0), "reset to main");
    // The merge commit that brought main in is not work to merge back.
    let no_ff = standing_after(|r, wt| {
        main_moves(r, "m1");
        in_tree(r, wt, &["merge", "-q", "--no-ff", "-m", "up", "main"]);
    });
    assert_eq!(no_ff.await, ("clean", 0), "--no-ff update");
    let work_and_update = standing_after(|r, wt| {
        tree_commits(r, wt, "w1");
        main_moves(r, "m1");
        in_tree(r, wt, &["merge", "-q", "-m", "up", "main"]);
    });
    assert_eq!(work_and_update.await, ("ready", 1), "only w1 to merge");
    // Work thrown away never reached main.
    let discarded = standing_after(|r, wt| {
        tree_commits(r, wt, "w1");
        in_tree(r, wt, &["reset", "-q", "--hard", "main"]);
    });
    assert_eq!(discarded.await, ("clean", 0), "work discarded");
}

#[tokio::test]
async fn work_that_reached_the_base_is_merged_however_it_got_there() {
    let merged = standing_after(|r, wt| {
        tree_commits(r, wt, "w1");
        r.git(&["merge", "-q", "--no-ff", "-m", "mg", "topic"]);
        main_moves(r, "m2");
    });
    assert_eq!(merged.await, ("merged", 0), "--no-ff merge");
    let ff = standing_after(|r, wt| {
        tree_commits(r, wt, "w1");
        r.git(&["merge", "-q", "topic"]);
    });
    assert_eq!(ff.await, ("merged", 0), "fast-forwarded into main");
    let rebased_then_merged = standing_after(|r, wt| {
        tree_commits(r, wt, "w1");
        main_moves(r, "m1");
        in_tree(r, wt, &["rebase", "-q", "main"]);
        r.git(&["merge", "-q", "--no-ff", "-m", "mg", "topic"]);
    });
    assert_eq!(rebased_then_merged.await, ("merged", 0), "rebased first");
    let amended = standing_after(|r, wt| {
        tree_commits(r, wt, "w1");
        in_tree(r, wt, &["commit", "-q", "--amend", "-m", "w1b"]);
        r.git(&["merge", "-q", "topic"]);
    });
    assert_eq!(amended.await, ("merged", 0), "amended first");
    let squashed = standing_after(|r, wt| {
        tree_commits(r, wt, "w1");
        tree_commits(r, wt, "w2");
        main_moves(r, "m1");
        r.git(&["merge", "-q", "--squash", "topic"]);
        r.git(&["commit", "-qm", "squash topic"]);
    });
    assert_eq!(squashed.await, ("merged", 2), "squash merge");
    let picked = standing_after(|r, wt| {
        tree_commits(r, wt, "w1");
        main_moves(r, "m1");
        r.git(&["cherry-pick", "topic"]);
    });
    assert_eq!(picked.await, ("merged", 1), "rebase merge / cherry-pick");
    // Main changed the same file again after the squash: not told apart
    // from work still to merge.
    let changed_after = standing_after(|r, wt| {
        tree_commits(r, wt, "w1");
        r.git(&["merge", "-q", "--squash", "topic"]);
        r.git(&["commit", "-qm", "squash topic"]);
        r.write("w1", "later\n");
        r.commit_all("later");
    });
    assert_eq!(
        changed_after.await,
        ("ready", 1),
        "changed after the squash"
    );
    let more_work = standing_after(|r, wt| {
        tree_commits(r, wt, "w1");
        r.git(&["merge", "-q", "--no-ff", "-m", "mg", "topic"]);
        tree_commits(r, wt, "w2");
    });
    assert_eq!(more_work.await, ("ready", 1), "work after the merge");
}
