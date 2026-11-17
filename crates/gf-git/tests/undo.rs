//! Undo / redo against real repositories: capture a scope, run the
//! operation, capture again, then restore either side.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};

use common::open;
use gf_git::{Capture, CommitOptions, HeadMode, MergeStyle, Repository, Scope, Side, StashOptions};
use gf_test_support::TestRepo;

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

fn read(repo: &TestRepo, rel: &str) -> Option<String> {
    std::fs::read_to_string(repo.path().join(rel)).ok()
}

/// Runs `op` between two captures of `scope`.
async fn recorded<F, Fut>(r: &Repository, scope: &Scope, op: F) -> (Capture, Capture)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = gf_core::Result<()>>,
{
    let before = r.capture(scope).await.unwrap();
    op().await.unwrap();
    let after = r.capture(scope).await.unwrap();
    assert_ne!(before, after, "the operation changed nothing in scope");
    (before, after)
}

/// Undo then redo, checking the guard each time.
async fn undo_redo(
    r: &Repository,
    scope: &Scope,
    (before, after): &(Capture, Capture),
    mode: HeadMode,
) {
    assert!(after.matches(&r.capture(scope).await.unwrap()));
    r.restore(scope, Some(after), before, mode).await.unwrap();
    assert!(before.matches(&r.capture(scope).await.unwrap()), "undo");
    r.restore(scope, Some(before), after, mode).await.unwrap();
    assert!(after.matches(&r.capture(scope).await.unwrap()), "redo");
    r.restore(scope, Some(after), before, mode).await.unwrap();
}

#[tokio::test]
async fn discarded_files_and_untracked_folders_come_back() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.write("a.txt", "edited\n");
    repo.write("build/out/x.bin", "x");
    repo.write("build/y", "y");
    let r = open(&repo).await;

    let mut scope = Scope {
        files: vec![p("a.txt"), p("build")],
        ..Scope::default()
    };
    scope.expand_files(repo.path()).unwrap();
    let rec = recorded(&r, &scope, || async {
        r.discard(&[p("a.txt")], &[p("build")]).await
    })
    .await;
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("a\n"));
    assert!(!repo.path().join("build").exists());

    undo_redo(&r, &scope, &rec, HeadMode::Keep).await;
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("edited\n"));
    assert_eq!(read(&repo, "build/out/x.bin").as_deref(), Some("x"));
    assert_eq!(read(&repo, "build/y").as_deref(), Some("y"));
}

#[tokio::test]
async fn a_changed_file_fails_the_guard() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.write("a.txt", "edited\n");
    let r = open(&repo).await;
    let scope = Scope {
        files: vec![p("a.txt")],
        ..Scope::default()
    };
    let (_, after) = recorded(&r, &scope, || async { r.discard(&[p("a.txt")], &[]).await }).await;
    repo.write("a.txt", "typed later\n");
    let now = r.capture(&scope).await.unwrap();
    assert_eq!(after.differences(&now), ["a.txt changed"]);
}

#[cfg(unix)]
#[tokio::test]
async fn executable_bit_survives() {
    use std::os::unix::fs::PermissionsExt;
    let repo = TestRepo::new();
    repo.write("README", "r\n");
    repo.commit_all("init");
    let script = repo.write("run.sh", "#!/bin/sh\n");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let r = open(&repo).await;
    let scope = Scope {
        files: vec![p("run.sh")],
        ..Scope::default()
    };
    let (before, _) = recorded(&r, &scope, || async {
        r.discard(&[], &[p("run.sh")]).await
    })
    .await;
    r.restore(&scope, None, &before, HeadMode::Keep)
        .await
        .unwrap();
    let mode = std::fs::metadata(&script).unwrap().permissions().mode();
    assert_ne!(mode & 0o111, 0);
}

#[tokio::test]
async fn staging_is_undone_exactly() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.commit_all("init");
    repo.write("a.txt", "A\n");
    repo.write("new.txt", "n\n");
    std::fs::remove_file(repo.path().join("b.txt")).unwrap();
    let r = open(&repo).await;
    let scope = Scope {
        index: vec![p("a.txt"), p("b.txt"), p("new.txt")],
        ..Scope::default()
    };
    let rec = recorded(&r, &scope, || r.stage_all()).await;
    undo_redo(&r, &scope, &rec, HeadMode::Keep).await;
    assert!(repo.git(&["diff", "--cached", "--name-only"]).is_empty());
    // Work tree untouched.
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("A\n"));
}

#[tokio::test]
async fn resolving_a_conflict_can_be_undone() {
    let repo = TestRepo::new();
    repo.write("f.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "other"]);
    repo.write("f.txt", "theirs\n");
    repo.commit_all("theirs");
    repo.git(&["switch", "-q", "main"]);
    repo.write("f.txt", "ours\n");
    repo.commit_all("ours");
    let r = open(&repo).await;
    r.merge("other", MergeStyle::Default).await.unwrap();
    let conflicted = read(&repo, "f.txt").unwrap();
    assert!(conflicted.contains("<<<<<<<"));

    let scope = Scope {
        index: vec![p("f.txt")],
        files: vec![p("f.txt")],
        ..Scope::default()
    };
    let (before, _) = recorded(&r, &scope, || r.take_side(Path::new("f.txt"), Side::Theirs)).await;
    assert!(repo.git(&["ls-files", "-u"]).is_empty(), "resolved");
    r.restore(&scope, None, &before, HeadMode::Keep)
        .await
        .unwrap();
    assert_eq!(
        repo.git(&["ls-files", "-u"]).lines().count(),
        3,
        "conflict stages back"
    );
    assert_eq!(read(&repo, "f.txt").unwrap(), conflicted);
}

#[tokio::test]
async fn undoing_a_commit_keeps_its_changes_staged() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    let first = repo.commit_all("init");
    repo.write("a.txt", "b\n");
    repo.git(&["add", "a.txt"]);
    let r = open(&repo).await;
    let scope = Scope::head();
    let rec = recorded(&r, &scope, || async {
        r.commit("second", &CommitOptions::default())
            .await
            .map(|_| ())
    })
    .await;
    undo_redo(&r, &scope, &rec, HeadMode::Soft).await;
    assert_eq!(repo.head(), first);
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "a.txt");
}

#[tokio::test]
async fn undoing_a_commit_on_a_detached_head_keeps_its_changes_staged() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    let first = repo.commit_all("init");
    repo.git(&["switch", "-q", "--detach"]);
    repo.write("a.txt", "b\n");
    repo.git(&["add", "a.txt"]);
    let r = open(&repo).await;
    let scope = Scope::head();
    let rec = recorded(&r, &scope, || async {
        r.commit("second", &CommitOptions::default())
            .await
            .map(|_| ())
    })
    .await;
    undo_redo(&r, &scope, &rec, HeadMode::Soft).await;
    assert_eq!(repo.head(), first);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("b\n"));
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "a.txt");
}

#[tokio::test]
async fn restore_refuses_when_things_changed_since_the_check() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.write("a.txt", "edited\n");
    let r = open(&repo).await;
    let scope = Scope {
        files: vec![p("a.txt")],
        ..Scope::default()
    };
    let (before, after) =
        recorded(&r, &scope, || async { r.discard(&[p("a.txt")], &[]).await }).await;
    repo.write("a.txt", "typed while the dialog was open\n");
    let err = r
        .restore(&scope, Some(&after), &before, HeadMode::Keep)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("a.txt changed"), "{err}");
    assert_eq!(
        read(&repo, "a.txt").as_deref(),
        Some("typed while the dialog was open\n")
    );
}

#[tokio::test]
async fn restore_changes_nothing_when_a_saved_copy_is_gone() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.write("b.txt", "b\n");
    let r = open(&repo).await;
    let scope = Scope {
        files: vec![p("a.txt"), p("b.txt")],
        ..Scope::default()
    };
    let (mut before, _) = recorded(&r, &scope, || async {
        std::fs::remove_file(repo.path().join("b.txt"))?;
        std::fs::write(repo.path().join("a.txt"), "z\n")?;
        Ok(())
    })
    .await;
    // As if `git gc` had pruned the copy of b.txt.
    before.files[1].1 = Some(gf_git::FileContent::Blob {
        oid: "1".repeat(40),
        executable: false,
    });
    assert!(
        r.restore(&scope, None, &before, HeadMode::Keep)
            .await
            .is_err()
    );
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("z\n"), "untouched");
}

#[cfg(unix)]
#[tokio::test]
async fn odd_file_names_are_saved_too() {
    let repo = TestRepo::new();
    repo.write("README", "r\n");
    repo.commit_all("init");
    repo.write("we\nird", "1");
    repo.write("\"quoted\"", "2");
    let r = open(&repo).await;
    let scope = Scope {
        files: vec![p("we\nird"), p("\"quoted\"")],
        ..Scope::default()
    };
    let rec = recorded(&r, &scope, || async {
        r.discard(&[], &[p("we\nird"), p("\"quoted\"")]).await
    })
    .await;
    undo_redo(&r, &scope, &rec, HeadMode::Keep).await;
    assert_eq!(read(&repo, "we\nird").as_deref(), Some("1"));
    assert_eq!(read(&repo, "\"quoted\"").as_deref(), Some("2"));
}

#[tokio::test]
async fn branch_config_of_a_longer_name_is_left_alone() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.git(&["branch", "x"]);
    repo.git(&["branch", "x.y"]);
    repo.git(&["config", "branch.x.remote", "origin"]);
    repo.git(&["config", "branch.x.y.remote", "origin"]);
    // A key with no value means true.
    std::fs::write(
        repo.path().join(".git/config"),
        std::fs::read_to_string(repo.path().join(".git/config")).unwrap()
            + "[branch \"x\"]\n\trebase\n",
    )
    .unwrap();
    let r = open(&repo).await;
    let scope = Scope {
        refs: vec!["refs/heads/x".into()],
        config: vec!["branch.x".into()],
        ..Scope::default()
    };
    let rec = recorded(&r, &scope, || r.delete_branch("x", true)).await;
    undo_redo(&r, &scope, &rec, HeadMode::Keep).await;
    assert_eq!(
        repo.git(&["config", "--get-all", "branch.x.y.remote"]),
        "origin"
    );
    assert_eq!(repo.git(&["config", "--bool", "branch.x.rebase"]), "true");
}

#[tokio::test]
async fn undoing_the_first_commit_unborns_the_branch() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.git(&["add", "a.txt"]);
    let r = open(&repo).await;
    let scope = Scope::head();
    let (before, _) = recorded(&r, &scope, || async {
        r.commit("first", &CommitOptions::default())
            .await
            .map(|_| ())
    })
    .await;
    r.restore(&scope, None, &before, HeadMode::Soft)
        .await
        .unwrap();
    assert!(
        !repo
            .git_output(&["rev-parse", "-q", "--verify", "HEAD"])
            .status
            .success()
    );
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "a.txt");
}

#[tokio::test]
async fn deleted_branch_comes_back_with_its_upstream() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.git(&["branch", "feature/x"]);
    repo.git(&["config", "branch.feature/x.remote", "origin"]);
    repo.git(&["config", "branch.feature/x.merge", "refs/heads/feature/x"]);
    let r = open(&repo).await;
    let scope = Scope {
        refs: vec!["refs/heads/feature/x".into()],
        config: vec!["branch.feature/x".into()],
        ..Scope::default()
    };
    let rec = recorded(&r, &scope, || r.delete_branch("feature/x", true)).await;
    undo_redo(&r, &scope, &rec, HeadMode::Keep).await;
    assert_eq!(repo.git(&["config", "branch.feature/x.remote"]), "origin");
}

#[tokio::test]
async fn renaming_the_current_branch_is_undone() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    let r = open(&repo).await;
    let scope = Scope {
        head: true,
        refs: vec!["refs/heads/main".into(), "refs/heads/trunk".into()],
        config: vec!["branch.main".into(), "branch.trunk".into()],
        ..Scope::default()
    };
    let rec = recorded(&r, &scope, || r.rename_branch("main", "trunk")).await;
    undo_redo(&r, &scope, &rec, HeadMode::Keep).await;
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "main");
    assert!(
        !repo
            .git_output(&["rev-parse", "-q", "--verify", "refs/heads/trunk"])
            .status
            .success()
    );
}

#[tokio::test]
async fn checkout_goes_back_to_the_previous_branch() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.git(&["switch", "-q", "-c", "dev"]);
    repo.write("a.txt", "dev\n");
    repo.commit_all("dev");
    repo.git(&["switch", "-q", "main"]);
    let r = open(&repo).await;
    let scope = Scope::head();
    let rec = recorded(&r, &scope, || r.switch_branch("dev", false)).await;
    undo_redo(&r, &scope, &rec, HeadMode::Keep).await;
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("a\n"));
}

#[tokio::test]
async fn stash_push_and_drop_are_undone() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.write("a.txt", "work\n");
    repo.write("new.txt", "untracked\n");
    let r = open(&repo).await;
    let mut scope = Scope {
        stash: true,
        index: vec![p("a.txt"), p("new.txt")],
        files: vec![p("a.txt"), p("new.txt")],
        ..Scope::default()
    };
    scope.expand_files(repo.path()).unwrap();
    let opts = StashOptions {
        message: Some("wip".into()),
        include_untracked: true,
        keep_index: false,
    };
    let rec = recorded(&r, &scope, || r.stash_push(&opts)).await;
    undo_redo(&r, &scope, &rec, HeadMode::Keep).await;
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("work\n"));
    assert_eq!(read(&repo, "new.txt").as_deref(), Some("untracked\n"));
    assert!(repo.git(&["stash", "list"]).is_empty());

    // Drop: the entry comes back with its message.
    r.stash_push(&opts).await.unwrap();
    let scope = Scope {
        stash: true,
        ..Scope::default()
    };
    let (before, _) = recorded(&r, &scope, || r.stash_drop(0)).await;
    r.restore(&scope, None, &before, HeadMode::Keep)
        .await
        .unwrap();
    assert!(repo.git(&["stash", "list"]).contains("wip"));
}

#[tokio::test]
async fn deleted_annotated_tag_keeps_its_message() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.git(&["tag", "-a", "v1", "-m", "release one"]);
    let tag_object = repo.git(&["rev-parse", "v1"]);
    let r = open(&repo).await;
    let scope = Scope {
        refs: vec!["refs/tags/v1".into()],
        ..Scope::default()
    };
    let rec = recorded(&r, &scope, || r.delete_tag("v1")).await;
    undo_redo(&r, &scope, &rec, HeadMode::Keep).await;
    assert_eq!(repo.git(&["rev-parse", "v1"]), tag_object);
    assert!(repo.git(&["tag", "-n1", "v1"]).contains("release one"));
}

#[tokio::test]
async fn submodule_discard_is_undone() {
    let sub = TestRepo::new();
    sub.write("lib.txt", "one\n");
    let first = sub.commit_all("one");
    sub.write("lib.txt", "two\n");
    sub.commit_all("two");
    let repo = TestRepo::new();
    repo.write("README.md", "hi\n");
    repo.commit_all("init");
    let url = sub.path().to_str().unwrap().to_owned();
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        &url,
        "lib",
    ]);
    repo.commit_all("add lib");
    let out = repo
        .command()
        .current_dir(repo.path().join("lib"))
        .args(["checkout", "-q", &first])
        .output()
        .unwrap();
    assert!(out.status.success());

    let r = open(&repo).await;
    let scope = Scope {
        submodules: vec![p("lib")],
        ..Scope::default()
    };
    let rec = recorded(&r, &scope, || async { r.discard(&[p("lib")], &[]).await }).await;
    undo_redo(&r, &scope, &rec, HeadMode::Keep).await;
    let head = repo
        .command()
        .current_dir(repo.path().join("lib"))
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), first);
}

#[tokio::test]
async fn leaving_changes_behind_stashes_switches_and_undoes_in_one_step() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.git(&["switch", "-q", "-c", "dev"]);
    repo.write("a.txt", "dev\n");
    repo.commit_all("dev");
    repo.git(&["switch", "-q", "main"]);
    repo.write("a.txt", "work\n");
    repo.write("new.txt", "untracked\n");
    let r = open(&repo).await;

    // a.txt is different on dev: a plain switch would be refused.
    let changed = [p("a.txt"), p("new.txt")];
    assert_eq!(
        r.switch_blockers("dev", &changed).await.unwrap(),
        [p("a.txt")]
    );
    assert!(r.switch_branch("dev", false).await.is_err());

    let mut scope = Scope {
        head: true,
        stash: true,
        index: changed.to_vec(),
        files: changed.to_vec(),
        ..Scope::default()
    };
    scope.expand_files(repo.path()).unwrap();
    let rec = recorded(&r, &scope, || r.stash_and_switch("main", "dev", false)).await;
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "dev");
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("dev\n"));
    assert!(read(&repo, "new.txt").is_none(), "untracked file left too");
    assert!(repo.git(&["stash", "list"]).contains("gf: left on main"));

    undo_redo(&r, &scope, &rec, HeadMode::Keep).await;
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("work\n"));
    assert_eq!(read(&repo, "new.txt").as_deref(), Some("untracked\n"));
    assert!(repo.git(&["stash", "list"]).is_empty());
}

#[tokio::test]
async fn a_failed_switch_puts_the_left_changes_back() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.write("a.txt", "work\n");
    let r = open(&repo).await;
    assert!(
        r.stash_and_switch("main", "no-such-branch", false)
            .await
            .is_err()
    );
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("work\n"));
    assert!(repo.git(&["stash", "list"]).is_empty());
}
