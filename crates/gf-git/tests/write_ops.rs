#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use common::{open, repo_with_merge};
use gf_core::model::{FileChange, StatusEntry};
use gf_git::{
    CommitOptions, DiffOptions, DiffTarget, LogOrder, LogQuery, PullMode, PushOptions, RunOptions,
    StashOptions, graph, patch,
};
use gf_test_support::TestRepo;

fn paths(p: &[&str]) -> Vec<PathBuf> {
    p.iter().map(PathBuf::from).collect()
}

// ---------------------------------------------------------------- index

#[tokio::test]
async fn discard_puts_a_submodule_back_on_its_recorded_commit() {
    let sub = TestRepo::new();
    sub.write("lib.txt", "one\n");
    let first = sub.commit_all("one");
    sub.write("lib.txt", "two\n");
    let recorded = sub.commit_all("two");

    let repo = TestRepo::new();
    repo.write("README.md", "hi\n");
    repo.commit_all("init");
    repo.git(
        &["-c", "protocol.file.allow=always", "submodule", "add", "-q"]
            .into_iter()
            .chain([sub.path().to_str().unwrap(), "lib"])
            .collect::<Vec<_>>(),
    );
    repo.commit_all("add lib");

    // Another commit checked out in the submodule, plus an edit in it.
    let in_sub = |args: &[&str]| {
        let out = repo
            .command()
            .current_dir(repo.path().join("lib"))
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    in_sub(&["checkout", "-q", &first]);
    repo.write("lib/lib.txt", "edited\n");
    repo.write("README.md", "changed\n");

    let r = open(&repo).await;
    r.discard(&paths(&["lib", "README.md"]), &[]).await.unwrap();
    assert_eq!(in_sub(&["rev-parse", "HEAD"]), recorded);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("lib/lib.txt")).unwrap(),
        "two\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("README.md")).unwrap(),
        "hi\n"
    );
    assert!(repo.git(&["status", "--porcelain"]).is_empty());
}

#[tokio::test]
async fn stage_unstage_and_discard() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.commit_all("init");
    repo.write("a.txt", "A\n");
    std::fs::remove_file(repo.path().join("b.txt")).unwrap();
    repo.write("new.txt", "n\n");
    let r = open(&repo).await;

    r.stage(&paths(&["a.txt", "b.txt"])).await.unwrap();
    let status = r.status().await.unwrap();
    assert!(
        status.entries.iter().filter(|e| e.is_staged()).count() == 2,
        "deletion staged too"
    );

    r.unstage(&paths(&["a.txt"])).await.unwrap();
    let staged: Vec<_> = r
        .status()
        .await
        .unwrap()
        .entries
        .into_iter()
        .filter(StatusEntry::is_staged)
        .collect();
    assert_eq!(staged.len(), 1);

    r.discard(&paths(&["a.txt"]), &paths(&["new.txt"]))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a\n"
    );
    assert!(!repo.path().join("new.txt").exists());

    r.stage_all().await.unwrap();
    r.unstage_all().await.unwrap();
    assert!(
        !r.status()
            .await
            .unwrap()
            .entries
            .iter()
            .any(StatusEntry::is_staged)
    );
}

#[tokio::test]
async fn unstage_before_first_commit() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    let r = open(&repo).await;
    r.stage(&paths(&["a.txt"])).await.unwrap();
    r.unstage(&paths(&["a.txt"])).await.unwrap();
    let entries = r.status().await.unwrap().entries;
    assert!(matches!(
        entries.as_slice(),
        [StatusEntry::Untracked { .. }]
    ));
}

#[tokio::test]
async fn partial_staging_with_patches() {
    let repo = TestRepo::new();
    let original: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    repo.write("f.txt", &original);
    repo.commit_all("init");
    // Two separate hunks: change line 2 and line 18.
    let changed = original
        .replace("line 2\n", "LINE 2\n")
        .replace("line 18\n", "LINE 18\n");
    repo.write("f.txt", &changed);
    let r = open(&repo).await;

    let wt = r
        .diff(&DiffTarget::WorkTree, &DiffOptions::default())
        .await
        .unwrap();
    assert_eq!(wt[0].hunks.len(), 2);
    assert!(patch::supports_partial(&wt[0]));

    // Stage only the first hunk.
    let p = patch::hunk_patch(&wt[0], 0, None, false).unwrap();
    r.apply_patch(p, true, false).await.unwrap();
    let staged = repo.git(&["diff", "--cached"]);
    assert!(
        staged.contains("+LINE 2") && !staged.contains("+LINE 18"),
        "{staged}"
    );

    // Stage just the added line of the second hunk: "-line 18" stays context.
    let wt = r
        .diff(&DiffTarget::WorkTree, &DiffOptions::default())
        .await
        .unwrap();
    let h = &wt[0].hunks[0];
    let added = h.lines.iter().position(|l| l.content == "LINE 18").unwrap();
    let p = patch::hunk_patch(&wt[0], 0, Some(&BTreeSet::from([added])), false).unwrap();
    r.apply_patch(p, true, false).await.unwrap();
    let index_content = repo.git(&["show", ":f.txt"]);
    assert!(
        index_content.contains("line 18\nLINE 18"),
        "both lines in index: {index_content}"
    );

    // Unstage the whole first staged hunk again (reverse patch on the index).
    let staged = r
        .diff(&DiffTarget::Staged, &DiffOptions::default())
        .await
        .unwrap();
    let p = patch::hunk_patch(&staged[0], 0, None, true).unwrap();
    r.apply_patch(p, true, true).await.unwrap();
    assert!(!repo.git(&["diff", "--cached"]).contains("LINE 2"));

    // Discard the remaining work tree hunk for line 2.
    let wt = r
        .diff(&DiffTarget::WorkTree, &DiffOptions::default())
        .await
        .unwrap();
    let first = wt[0]
        .hunks
        .iter()
        .position(|h| h.lines.iter().any(|l| l.content == "LINE 2"))
        .unwrap();
    let p = patch::hunk_patch(&wt[0], first, None, true).unwrap();
    r.apply_patch(p, false, true).await.unwrap();
    let on_disk = std::fs::read_to_string(repo.path().join("f.txt")).unwrap();
    assert!(on_disk.contains("line 2\n") && !on_disk.contains("LINE 2"));
}

#[tokio::test]
async fn untracked_file_diff() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.write("dir/new file.txt", "one\ntwo\n");
    let r = open(&repo).await;
    let d = r
        .diff(
            &DiffTarget::Untracked("dir/new file.txt".into()),
            &DiffOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].change, FileChange::Added);
    assert_eq!(
        d[0].new_path.as_deref(),
        Some(Path::new("dir/new file.txt"))
    );
    assert_eq!(d[0].line_stats(), (2, 0));
}

// ---------------------------------------------------------------- commit

#[tokio::test]
async fn commit_and_amend() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    let r = open(&repo).await;
    assert_eq!(r.head_message().await.unwrap(), None);

    r.stage_all().await.unwrap();
    let msg = "feat: first\n\n#123 keeps the hash line\n";
    let id = r.commit(msg, &CommitOptions::default()).await.unwrap();
    assert_eq!(id.as_str(), repo.head());
    assert_eq!(
        r.head_message().await.unwrap().as_deref(),
        Some("feat: first\n\n#123 keeps the hash line")
    );

    let amended = r
        .commit(
            "feat: first (amended)",
            &CommitOptions {
                amend: true,
                ..CommitOptions::default()
            },
        )
        .await
        .unwrap();
    assert_ne!(amended, id);
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");

    let empty = r.commit("nothing", &CommitOptions::default()).await;
    assert!(empty.is_err(), "nothing staged must fail");

    // Amending with an empty message adds staged changes, keeping the message.
    repo.write("b.txt", "b\n");
    r.stage_all().await.unwrap();
    r.commit(
        "",
        &CommitOptions {
            amend: true,
            ..CommitOptions::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        r.head_message().await.unwrap().as_deref(),
        Some("feat: first (amended)")
    );
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
    assert!(
        repo.git(&["ls-tree", "--name-only", "HEAD"])
            .contains("b.txt")
    );
}

// ---------------------------------------------------------------- branches & tags

#[tokio::test]
async fn branch_lifecycle() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    let r = open(&repo).await;

    assert!(r.check_branch_name("feature/ok").await.unwrap().is_none());
    assert!(r.check_branch_name("bad..name").await.unwrap().is_some());
    assert!(r.check_branch_name(" ").await.unwrap().is_some());

    r.create_branch("feature/x", None, true).await.unwrap();
    assert_eq!(
        r.status().await.unwrap().head.branch.as_deref(),
        Some("feature/x")
    );
    repo.write("x.txt", "x\n");
    repo.commit_all("x work");

    r.switch_branch("main", false).await.unwrap();
    assert!(
        r.delete_branch("feature/x", false).await.is_err(),
        "unmerged branch is protected"
    );
    r.rename_branch("feature/x", "feature/y").await.unwrap();
    r.delete_branch("feature/y", true).await.unwrap();
    assert!(
        !r.branches()
            .await
            .unwrap()
            .iter()
            .any(|b| b.name.short().starts_with("feature"))
    );

    let head = repo.head();
    r.checkout_detached(&head).await.unwrap();
    assert_eq!(r.status().await.unwrap().head.branch, None);
}

#[tokio::test]
async fn tracking_branch_from_remote() {
    let origin = repo_with_merge();
    let clone = origin.clone_repo();
    let r = open(&clone).await;
    r.switch_branch("origin/feature", true).await.unwrap();
    let head = r.status().await.unwrap().head;
    assert_eq!(head.branch.as_deref(), Some("feature"));
    assert_eq!(head.upstream.as_deref(), Some("origin/feature"));
}

#[tokio::test]
async fn tags() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    let r = open(&repo).await;
    r.create_tag("v1.0", None, Some("Release 1.0\n\nnotes"))
        .await
        .unwrap();
    r.create_tag("light", Some("HEAD"), None).await.unwrap();
    let tags = r.tags().await.unwrap();
    assert_eq!(tags.len(), 2);
    assert!(
        tags.iter()
            .any(|t| t.message.as_deref() == Some("Release 1.0\n\nnotes"))
    );
    r.delete_tag("light").await.unwrap();
    assert_eq!(r.tags().await.unwrap().len(), 1);
}

// ---------------------------------------------------------------- stash

#[tokio::test]
async fn stash_round_trip() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.write("a.txt", "changed\n");
    repo.write("u.txt", "untracked\n");
    let r = open(&repo).await;

    r.stash_push(&StashOptions {
        message: Some("wip login".into()),
        include_untracked: true,
        keep_index: false,
        paths: Vec::new(),
    })
    .await
    .unwrap();
    assert!(
        r.status().await.unwrap().entries.is_empty(),
        "tree is clean after stashing"
    );
    let list = r.stashes().await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].message.contains("wip login"));

    r.stash_apply(0).await.unwrap();
    assert_eq!(r.stashes().await.unwrap().len(), 1, "apply keeps the entry");
    repo.git(&["checkout", "--", "a.txt"]);
    std::fs::remove_file(repo.path().join("u.txt")).unwrap();

    r.stash_pop(0).await.unwrap();
    assert!(r.stashes().await.unwrap().is_empty());
    assert!(repo.path().join("u.txt").exists());

    r.stash_push(&StashOptions::default()).await.unwrap();
    r.stash_drop(0).await.unwrap();
    assert!(r.stashes().await.unwrap().is_empty());
}

// ---------------------------------------------------------------- sync

#[tokio::test]
async fn fetch_pull_push_with_a_bare_remote() {
    let remote = TestRepo::new_bare();
    let a = TestRepo::new();
    a.write("f.txt", "1\n");
    a.commit_all("one");
    a.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    let ra = open(&a).await;

    ra.push(
        &PushOptions {
            remote: Some("origin".into()),
            branch: Some("main".into()),
            set_upstream: true,
            ..PushOptions::default()
        },
        RunOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        ra.status().await.unwrap().head.upstream.as_deref(),
        Some("origin/main")
    );

    let b = remote.clone_repo();
    b.write("f.txt", "2\n");
    b.commit_all("two");
    b.git(&["push", "-q"]);

    ra.fetch(None, RunOptions::default()).await.unwrap();
    assert_eq!(ra.status().await.unwrap().head.behind, 1);
    ra.pull(PullMode::FastForwardOnly, RunOptions::default())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(a.path().join("f.txt")).unwrap(),
        "2\n"
    );

    // Diverge, then a lease-protected force push replaces the remote branch.
    a.write("f.txt", "3a\n");
    a.commit_all("three-a");
    b.git(&["pull", "-q"]);
    b.write("f.txt", "3b\n");
    b.commit_all("three-b");
    b.git(&["push", "-q"]);
    let plain = ra
        .push(&PushOptions::default(), RunOptions::default())
        .await;
    assert!(plain.is_err(), "non-fast-forward push is rejected");
    ra.fetch(Some("origin"), RunOptions::default())
        .await
        .unwrap();
    ra.push(
        &PushOptions {
            force_with_lease: true,
            ..PushOptions::default()
        },
        RunOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(remote.git(&["log", "-1", "--format=%s", "main"]), "three-a");
}

// ---------------------------------------------------------------- graph

#[tokio::test]
async fn graph_of_real_history() {
    let repo = repo_with_merge();
    let r = open(&repo).await;
    let commits = r
        .log(&LogQuery {
            order: LogOrder::Topo,
            ..LogQuery::default()
        })
        .await
        .unwrap();
    let rows = graph::layout(&commits, 6);
    assert_eq!(rows.len(), commits.len());
    assert!(
        rows[0].text().starts_with('◉'),
        "merge commit first: {:?}",
        rows[0].text()
    );
    assert!(rows.iter().any(|r| r.text().contains('╮')));
    assert!(rows.iter().any(|r| r.text().contains('╯')));
    assert_eq!(rows.last().unwrap().text(), "●");
}

#[tokio::test]
async fn numstat_for_worktree_and_index() {
    let repo = TestRepo::new();
    repo.write("a.txt", "1\n2\n3\n");
    repo.commit_all("init");
    repo.write("a.txt", "1\nTWO\n3\n4\n");
    repo.write("b.txt", "new\n");
    repo.git(&["add", "b.txt"]);
    let r = open(&repo).await;

    let wt = r.numstat(false).await.unwrap();
    assert_eq!(
        wt[Path::new("a.txt")],
        Some(gf_git::LineCounts {
            added: 2,
            removed: 1
        })
    );
    assert!(!wt.contains_key(Path::new("b.txt")));
    let staged = r.numstat(true).await.unwrap();
    assert_eq!(
        staged[Path::new("b.txt")],
        Some(gf_git::LineCounts {
            added: 1,
            removed: 0
        })
    );
}

#[tokio::test]
async fn unpushed_commits_of_an_unpublished_branch() {
    let remote = TestRepo::new_bare();
    let a = TestRepo::new();
    a.write("f.txt", "1\n");
    a.commit_all("one");
    a.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    a.git(&["push", "-q", "origin", "main"]);
    a.git(&["switch", "-q", "-c", "topic"]);
    a.write("f.txt", "2\n");
    a.commit_all("two");
    a.write("f.txt", "3\n");
    a.commit_all("three");
    let r = open(&a).await;
    assert_eq!(r.unpushed_count().await.unwrap(), 2);
    assert_eq!(
        open(&TestRepo::new()).await.unpushed_count().await.unwrap(),
        0
    );
}

#[tokio::test]
async fn one_file_can_be_stashed_and_patterns_ignored_once() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.commit_all("init");
    repo.write("a.txt", "changed a\n");
    repo.write("b.txt", "changed b\n");
    let r = open(&repo).await;

    r.stash_push(&StashOptions {
        paths: vec!["a.txt".into()],
        ..StashOptions::default()
    })
    .await
    .unwrap();
    let left: Vec<_> = r
        .status()
        .await
        .unwrap()
        .entries
        .iter()
        .map(|e| e.path().clone())
        .collect();
    assert_eq!(
        left,
        [std::path::PathBuf::from("b.txt")],
        "only a.txt stashed"
    );

    r.ignore("*.log").await.unwrap();
    r.ignore("*.log").await.unwrap();
    repo.write("x.log", "noise");
    let text = std::fs::read_to_string(repo.path().join(".gitignore")).unwrap();
    assert_eq!(text, "*.log\n", "added once");
}

#[tokio::test]
async fn a_missing_commit_graph_is_written_once() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    let r = open(&repo).await;
    assert!(r.write_commit_graph_if_missing().await.expect("first"));
    assert!(repo.path().join(".git/objects/info/commit-graph").exists());
    assert!(
        !r.write_commit_graph_if_missing().await.expect("second"),
        "already there"
    );
}
