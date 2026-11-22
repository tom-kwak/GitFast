//! Contract tests: for every read served by gix, the gix backend must give
//! exactly what the git CLI backend gives on the same repository. A
//! mismatch means `routes.rs` must keep that command on the CLI.
#![cfg(feature = "gix")]
#![allow(clippy::unwrap_used)]

mod common;

use std::fmt::Debug;
use std::future::Future;

use common::open;
use gf_core::model::Oid;
use gf_git::backend::{
    BranchOps, CliBackend, DiffOps, GixBackend, LogOps, RebaseOps, RefOps, ReflogOps, RemoteOps,
    StashOps, StatusOps, TagOps, WorktreeOps,
};
use gf_git::{LogOrder, LogQuery, Repository};
use gf_test_support::TestRepo;

/// Runs `f` on both backends and asserts equal results (errors compare by
/// "is an error" only).
async fn same<'a, T, F, Fut>(repo: &'a Repository, what: &str, f: F)
where
    T: PartialEq + Debug,
    F: Fn(Backend<'a>) -> Fut,
    Fut: Future<Output = gf_core::Result<T>>,
{
    let (cli, gix) = repo.backends();
    let gix = gix.expect("gix backend available");
    let a = f(Backend::Cli(cli)).await;
    let b = f(Backend::Gix(gix)).await;
    match (a, b) {
        (Ok(a), Ok(b)) => assert_eq!(b, a, "{what}: gix (left) differs from git CLI (right)"),
        (a, b) => panic!("{what}: cli {a:?} vs gix {b:?}"),
    }
}

#[derive(Clone, Copy)]
enum Backend<'a> {
    Cli(&'a CliBackend),
    Gix(&'a GixBackend),
}

/// Dispatches a trait call to whichever backend `b` is.
macro_rules! call {
    ($b:expr, $tr:ident :: $m:ident ( $($arg:expr),* )) => {
        match $b {
            Backend::Cli(c) => $tr::$m(c, $($arg),*).await,
            Backend::Gix(g) => $tr::$m(g, $($arg),*).await,
        }
    };
}

/// A clone of `origin` with: local commits ahead and remote commits behind,
/// a merged feature branch, a branch whose upstream is gone, a branch
/// tracking a local branch, tags (light and annotated), extra remotes,
/// stashes and a locked linked worktree.
fn fixture() -> (TestRepo, TestRepo, TestRepo) {
    let origin = TestRepo::new();
    origin.write("a.txt", "a\n");
    origin.commit_all("A: initial");
    origin.write("b.txt", "b\n");
    origin.commit_all("B: second\n\nWith a body.");

    let work = origin.clone_repo();
    // Upstream that will disappear.
    work.git(&["switch", "-q", "-c", "doomed"]);
    work.git(&["push", "-q", "-u", "origin", "doomed"]);
    work.git(&["switch", "-q", "main"]);
    origin.git(&["branch", "-D", "doomed"]);
    // Remote moves on: main is behind by one.
    origin.write("o.txt", "remote\n");
    origin.commit_all("O: remote work");
    work.git(&["fetch", "-q", "--prune"]);

    // Local work: a merged feature and two commits ahead.
    work.git(&["switch", "-q", "-c", "feature"]);
    work.write("f.txt", "f\n");
    work.commit_all("F1: feature");
    work.write("f.txt", "f2\n");
    work.commit_all("F2: more feature");
    work.git(&["switch", "-q", "main"]);
    work.write("c.txt", "c\n");
    work.commit_all("C: local");
    work.git(&[
        "merge",
        "-q",
        "--no-ff",
        "-m",
        "M: merge feature",
        "feature",
    ]);
    work.write("d.txt", "d\n");
    work.commit_all("D: after merge");

    work.git(&["branch", "--track", "follows-main", "main"]);
    work.git(&["tag", "v1", "HEAD~1"]);
    work.git(&["tag", "-a", "v2", "-m", "Release 2\n\nNotes line."]);

    work.git(&["remote", "add", "zeta", "https://example.com/zeta.git"]);
    work.git(&["remote", "add", "alpha", "git@example.com:team/alpha.git"]);
    work.git(&[
        "remote",
        "set-url",
        "--push",
        "alpha",
        "ssh://push.example.com/alpha.git",
    ]);

    work.write("a.txt", "stash one\n");
    work.git(&["stash", "push", "-q", "-m", "first stash"]);
    work.write("b.txt", "stash two\n");
    work.git(&["stash", "push", "-q"]);

    let linked = TestRepo::new_bare(); // only for a free temp dir path
    let wt = linked.path().join("wt");
    work.git(&["worktree", "add", "-q", wt.to_str().unwrap(), "feature"]);
    work.git(&["worktree", "lock", wt.to_str().unwrap()]);
    // gix walks topo order only with a commit-graph (git writes one on gc).
    work.git(&["commit-graph", "write", "--reachable"]);
    (origin, work, linked)
}

fn topo(revisions: &[&str]) -> LogQuery {
    LogQuery {
        revisions: revisions.iter().map(|r| (*r).to_owned()).collect(),
        order: LogOrder::Topo,
        ..LogQuery::default()
    }
}

#[tokio::test]
async fn reads_match_the_git_cli() {
    let (_origin, work, _linked) = fixture();
    let repo = open(&work).await;

    same(&repo, "head_exists", |b| async move {
        call!(b, LogOps::head_exists())
    })
    .await;
    same(&repo, "head_oid", |b| async move {
        call!(b, ReflogOps::head_oid())
    })
    .await;
    same(&repo, "branches", |b| async move {
        call!(b, RefOps::branches())
    })
    .await;
    same(&repo, "tags", |b| async move { call!(b, RefOps::tags()) }).await;
    same(&repo, "remotes", |b| async move {
        call!(b, RemoteOps::remotes())
    })
    .await;
    same(&repo, "unpushed_count", |b| async move {
        call!(b, RefOps::unpushed_count())
    })
    .await;
    same(&repo, "reflog", |b| async move {
        call!(b, ReflogOps::reflog(20))
    })
    .await;
    same(&repo, "stashes", |b| async move {
        call!(b, StashOps::stashes())
    })
    .await;
    same(&repo, "worktrees", |b| async move {
        call!(b, WorktreeOps::worktrees())
    })
    .await;

    let base: Oid = work.git(&["rev-parse", "HEAD~3"]).parse().unwrap();
    for since in [None, Some(&base)] {
        same(&repo, "unpushed", |b| async move {
            let mut ids = call!(b, RefOps::unpushed(since))?;
            ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            Ok(ids)
        })
        .await;
    }
    // Newest-first order is the same on both backends, so no sorting here.
    same(&repo, "unpushed_recent", |b| async move {
        call!(b, RefOps::unpushed_recent(2))
    })
    .await;
    same(&repo, "rebase_plan", |b| {
        let base = &base;
        async move { call!(b, RebaseOps::rebase_plan(Some(base))) }
    })
    .await;
    let ids: Vec<Oid> = ["HEAD", "HEAD~1"]
        .iter()
        .map(|r| work.git(&["rev-parse", r]).parse().unwrap())
        .collect();
    same(&repo, "squash_preview rejects merges", |b| {
        let ids = &ids;
        async move {
            let error = call!(b, RebaseOps::squash_preview(ids))
                .unwrap_err()
                .to_string();
            assert!(error.contains("includes merge commit"));
            Ok(error)
        }
    })
    .await;

    // The feature's two commits form a linear range with no merge to replay.
    work.git(&["switch", "--detach", "feature"]);
    let ids: Vec<Oid> = ["HEAD", "HEAD~1"]
        .iter()
        .map(|r| work.git(&["rev-parse", r]).parse().unwrap())
        .collect();
    same(&repo, "squash_preview", |b| {
        let ids = &ids;
        async move { call!(b, RebaseOps::squash_preview(ids)) }
    })
    .await;
}

#[tokio::test]
async fn log_walks_match_the_git_cli() {
    let (_origin, work, _linked) = fixture();
    let repo = open(&work).await;
    let queries = [
        LogQuery::default(),
        LogQuery::head(3),
        LogQuery {
            skip: 2,
            max_count: Some(3),
            ..LogQuery::default()
        },
        topo(&[]),
        topo(&["feature", "origin/main"]),
        topo(&["HEAD~3..HEAD"]),
        topo(&["main", "^feature"]),
        LogQuery {
            all_refs: true,
            ..topo(&[])
        },
        LogQuery {
            first_parent: true,
            ..topo(&[])
        },
        LogQuery {
            order: LogOrder::Date,
            all_refs: true,
            ..LogQuery::default()
        },
        LogQuery {
            first_parent: true,
            ..LogQuery::default()
        },
    ];
    for q in &queries {
        let what = format!("log {q:?}");
        same(&repo, &what, |b| async move { call!(b, LogOps::log(q)) }).await;
    }

    // Commits newer than the commit-graph (it is only rewritten on gc).
    work.write("late.txt", "late\n");
    work.commit_all("L1: after the graph");
    work.git(&["merge", "-q", "--no-ff", "-m", "L2: merge again", "feature"]);
    for q in &queries {
        let what = format!("log with a stale graph {q:?}");
        same(&repo, &what, |b| async move { call!(b, LogOps::log(q)) }).await;
    }

    // Without a commit-graph, topo order is left to git.
    let plain = TestRepo::new();
    plain.write("a", "a\n");
    plain.commit_all("a");
    let r = open(&plain).await;
    let (_, gix) = r.backends();
    assert!(matches!(
        LogOps::log(gix.unwrap(), &topo(&[])).await,
        Err(gf_core::GfError::Unsupported(_))
    ));
    assert_eq!(
        r.log(&topo(&[])).await.unwrap().len(),
        1,
        "routed to the CLI"
    );
}

#[tokio::test]
async fn detached_and_unborn_heads_match() {
    let (_origin, work, _linked) = fixture();
    work.git(&["switch", "-q", "--detach", "HEAD~1"]);
    let repo = open(&work).await;
    same(&repo, "detached branches", |b| async move {
        call!(b, RefOps::branches())
    })
    .await;
    same(&repo, "detached worktrees", |b| async move {
        call!(b, WorktreeOps::worktrees())
    })
    .await;

    let empty = TestRepo::new();
    let repo = open(&empty).await;
    same(&repo, "unborn head_exists", |b| async move {
        call!(b, LogOps::head_exists())
    })
    .await;
    same(&repo, "unborn log", |b| async move {
        call!(b, LogOps::log(&LogQuery::default()))
    })
    .await;
    same(&repo, "unborn branches", |b| async move {
        call!(b, RefOps::branches())
    })
    .await;
    same(&repo, "unborn unpushed", |b| async move {
        call!(b, RefOps::unpushed_count())
    })
    .await;
    same(&repo, "unborn stashes", |b| async move {
        call!(b, StashOps::stashes())
    })
    .await;
}

#[tokio::test]
async fn config_changes_are_seen() {
    let (_origin, work, _linked) = fixture();
    let repo = open(&work).await;
    let _ = repo.remotes().await.unwrap();
    work.git(&["remote", "add", "later", "https://example.com/later.git"]);
    same(&repo, "remotes after add", |b| async move {
        call!(b, RemoteOps::remotes())
    })
    .await;
}

/// Every kind of working tree change `git status` reports.
fn dirty() -> TestRepo {
    let repo = TestRepo::new();
    repo.write("keep.txt", "1\n2\n3\n");
    repo.write("modify.txt", "a\nb\nc\nd\n");
    repo.write("staged.txt", "s\n");
    repo.write("delete.txt", "gone\n");
    repo.write("unstage-delete.txt", "x\n");
    repo.write("rename-me.txt", "one\ntwo\nthree\nfour\nfive\n");
    repo.write("exec.sh", "echo\n");
    repo.write("newline.txt", "first\nlast\n");
    repo.write("bin.dat", "\0\x01\x02");
    repo.write("link-target.txt", "t\n");
    repo.write("dir/nested.txt", "n\n");
    repo.commit_all("base");

    repo.write("modify.txt", "a\nB\nc\nd\ne\n");
    repo.write("staged.txt", "s\nmore\n");
    repo.git(&["add", "staged.txt"]);
    repo.write("staged.txt", "s\nmore\nand worktree\n");
    repo.git(&["rm", "-q", "delete.txt"]);
    std::fs::remove_file(repo.path().join("unstage-delete.txt")).unwrap();
    repo.git(&["mv", "rename-me.txt", "renamed.txt"]);
    repo.write("renamed.txt", "one\ntwo\nthree\nfour\nfive\nsix\n");
    repo.git(&["update-index", "--chmod=+x", "exec.sh"]);
    repo.write("newline.txt", "first\nlast");
    repo.write("bin.dat", "\0\x01\x03\x04");
    repo.write("added.txt", "new\nfile\n");
    repo.git(&["add", "added.txt"]);
    repo.write("untracked.txt", "u\n");
    repo.write("newdir/deep/u.txt", "u\n");
    repo.write("dir/also-untracked.txt", "u\n");
    repo
}

#[tokio::test]
async fn status_and_numstat_match_the_git_cli() {
    let work = dirty();
    let repo = open(&work).await;
    same(
        &repo,
        "status",
        |b| async move { call!(b, StatusOps::status()) },
    )
    .await;
    for staged in [false, true] {
        same(&repo, &format!("numstat staged={staged}"), |b| async move {
            call!(b, DiffOps::numstat(staged))
        })
        .await;
    }

    // Upstream, ahead/behind in the status header.
    let (_origin, work, _linked) = fixture();
    work.write("a.txt", "changed\n");
    let repo = open(&work).await;
    same(&repo, "status with upstream", |b| async move {
        call!(b, StatusOps::status())
    })
    .await;
}

#[tokio::test]
async fn conflicted_and_empty_status_match() {
    let repo = TestRepo::new();
    repo.write("f.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "topic"]);
    repo.write("f.txt", "topic\n");
    repo.write("both.txt", "topic\n");
    repo.commit_all("topic");
    repo.git(&["switch", "-q", "main"]);
    repo.write("f.txt", "main\n");
    repo.write("both.txt", "main\n");
    repo.commit_all("main");
    let _ = repo.git_output(&["merge", "-q", "topic"]);
    let r = open(&repo).await;
    same(&r, "conflicted status", |b| async move {
        call!(b, StatusOps::status())
    })
    .await;

    let detached = TestRepo::new();
    detached.write("a", "a\n");
    detached.commit_all("a");
    detached.git(&["switch", "-q", "--detach"]);
    let r = open(&detached).await;
    same(&r, "detached status", |b| async move {
        call!(b, StatusOps::status())
    })
    .await;

    let clean = TestRepo::new();
    clean.write("a", "a\n");
    clean.commit_all("a");
    let r = open(&clean).await;
    same(&r, "clean status", |b| async move {
        call!(b, StatusOps::status())
    })
    .await;
    same(&r, "clean numstat", |b| async move {
        call!(b, DiffOps::numstat(false))
    })
    .await;
}

/// A superproject with one submodule per state: clean, checked out at
/// another commit, modified content, untracked content, a staged new
/// commit, and a newly added (staged) submodule.
fn with_submodules() -> (TestRepo, TestRepo) {
    let sub = TestRepo::new();
    sub.write("s.txt", "one\n");
    sub.commit_all("S1");
    sub.write("s.txt", "two\n");
    sub.commit_all("S2");
    let url = sub.path().to_str().unwrap().to_owned();

    let sup = TestRepo::new();
    sup.write("top.txt", "top\n");
    for name in ["clean", "moved", "modified", "untracked", "staged"] {
        sup.git(&[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &url,
            name,
        ]);
    }
    sup.commit_all("add submodules");
    sup.git(&["-C", "moved", "checkout", "-q", "HEAD~1"]);
    sup.write("modified/s.txt", "changed\n");
    sup.write("untracked/new.txt", "u\n");
    sup.git(&["-C", "staged", "checkout", "-q", "HEAD~1"]);
    sup.git(&["add", "staged"]);
    sup.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        &url,
        "added",
    ]);
    sup.write("top.txt", "top changed\n");
    (sub, sup)
}

#[tokio::test]
async fn submodule_status_and_numstat_match() {
    let (_sub, sup) = with_submodules();
    let repo = open(&sup).await;
    same(&repo, "status with submodules", |b| async move {
        call!(b, StatusOps::status())
    })
    .await;
    for staged in [false, true] {
        same(
            &repo,
            &format!("numstat with submodules staged={staged}"),
            |b| async move { call!(b, DiffOps::numstat(staged)) },
        )
        .await;
    }

    // A clone without `--recurse-submodules`: none are initialized.
    sup.git(&["commit", "-qam", "everything"]);
    let clone = sup.clone_repo();
    let repo = open(&clone).await;
    same(
        &repo,
        "status with uninitialized submodules",
        |b| async move { call!(b, StatusOps::status()) },
    )
    .await;
    same(
        &repo,
        "numstat with uninitialized submodules",
        |b| async move { call!(b, DiffOps::numstat(false)) },
    )
    .await;
}

// ------------------------------------------------------------------ writes

/// Refs, reflog entries (ids and messages; identity and time vary) and
/// local config: what a ref edit can change.
fn ref_state(repo: &TestRepo) -> (String, Vec<String>, String) {
    let refs = repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]);
    let logs_dir = repo.path().join(".git/logs");
    let mut logs = Vec::new();
    let mut stack = vec![logs_dir.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = p.strip_prefix(&logs_dir).unwrap().display().to_string();
            for line in std::fs::read_to_string(&p).unwrap().lines() {
                let (head, message) = line.split_once('\t').unwrap_or((line, ""));
                let ids: Vec<&str> = head.split(' ').take(2).collect();
                logs.push(format!("{name}: {} {message}", ids.join(" ")));
            }
        }
    }
    logs.sort();
    let config = repo.git(&["config", "--local", "--list"]);
    (refs, logs, config)
}

/// Two identical repositories (fixed dates make identical ids).
fn twins() -> (TestRepo, TestRepo) {
    let make = || {
        let repo = common::repo_with_merge();
        repo.git(&["tag", "v1", "HEAD~1"]);
        repo.git(&["switch", "-q", "-c", "unmerged"]);
        repo.write("u.txt", "u\n");
        repo.commit_all("U: not merged");
        repo.git(&["switch", "-q", "main"]);
        repo
    };
    (make(), make())
}

#[tokio::test]
async fn ref_edits_match_the_git_cli() {
    type Edit = for<'a> fn(
        Backend<'a>,
    )
        -> std::pin::Pin<Box<dyn Future<Output = gf_core::Result<()>> + 'a>>;
    let edits: Vec<(&str, Edit)> = vec![
        ("create from HEAD", |b| {
            Box::pin(async move { call!(b, BranchOps::create_branch("topic", None, false)) })
        }),
        ("create from a branch", |b| {
            Box::pin(async move {
                call!(
                    b,
                    BranchOps::create_branch("from-feature", Some("feature"), false)
                )
            })
        }),
        ("create from a tag", |b| {
            Box::pin(
                async move { call!(b, BranchOps::create_branch("from-tag", Some("v1"), false)) },
            )
        }),
        ("delete merged", |b| {
            Box::pin(async move { call!(b, BranchOps::delete_branch("feature", false)) })
        }),
        ("force delete", |b| {
            Box::pin(async move { call!(b, BranchOps::delete_branch("unmerged", true)) })
        }),
        ("lightweight tag", |b| {
            Box::pin(async move { call!(b, TagOps::create_tag("lt", Some("HEAD~1"), None)) })
        }),
        ("tag of a tag", |b| {
            Box::pin(async move { call!(b, TagOps::create_tag("lt2", Some("v1"), None)) })
        }),
        ("delete tag", |b| {
            Box::pin(async move { call!(b, TagOps::delete_tag("v1")) })
        }),
    ];
    for (what, edit) in edits {
        let (a, b) = twins();
        let (ra, rb) = (open(&a).await, open(&b).await);
        let (cli, _) = ra.backends();
        let (_, gix) = rb.backends();
        edit(Backend::Cli(cli)).await.unwrap();
        edit(Backend::Gix(gix.unwrap()))
            .await
            .unwrap_or_else(|e| panic!("{what}: gix failed: {e}"));
        assert_eq!(
            ref_state(&b),
            ref_state(&a),
            "{what}: gix (left) vs git CLI (right)"
        );
    }
}

#[tokio::test]
async fn ref_edits_git_must_do_are_left_to_the_cli() {
    let (repo, _) = twins();
    repo.git(&["remote", "add", "origin", "https://example.com/x.git"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["branch", "--track", "tracked", "main"]);
    let r = open(&repo).await;
    let (_, gix) = r.backends();
    let gix = gix.unwrap();
    let unsupported =
        |res: gf_core::Result<()>| matches!(res, Err(gf_core::GfError::Unsupported(_)));
    assert!(
        unsupported(BranchOps::delete_branch(gix, "unmerged", false).await),
        "not merged"
    );
    assert!(
        unsupported(BranchOps::delete_branch(gix, "main", true).await),
        "checked out"
    );
    assert!(
        unsupported(BranchOps::delete_branch(gix, "tracked", true).await),
        "has config"
    );
    assert!(
        unsupported(BranchOps::create_branch(gix, "x", Some("origin/main"), false).await),
        "tracking"
    );
    assert!(
        unsupported(BranchOps::create_branch(gix, "x", None, true).await),
        "checkout"
    );
    assert!(
        unsupported(BranchOps::rename_branch(gix, "feature", "y").await),
        "rename"
    );
    assert!(
        unsupported(TagOps::create_tag(gix, "a", None, Some("msg")).await),
        "annotated"
    );

    repo.write(".git/hooks/reference-transaction", "#!/bin/sh\nexit 0\n");
    assert!(
        unsupported(BranchOps::create_branch(gix, "hooked", None, false).await),
        "hook"
    );
    // Through the repository, the CLI does it instead.
    r.create_branch("hooked", None, false).await.unwrap();
    assert!(repo.git(&["branch", "--list", "hooked"]).contains("hooked"));
}

#[tokio::test]
async fn gix_edits_appear_in_the_command_log() {
    let (repo, _) = twins();
    let bus = gf_core::EventBus::new(16);
    let mut events = bus.subscribe();
    let runner = common::runner(&repo).with_event_bus(bus);
    let r = Repository::open(repo.path(), runner).await.unwrap();
    r.create_branch("logged", None, false).await.unwrap();
    let mut started = Vec::new();
    while let Ok(gf_core::CoreEvent::Command(e)) = events.try_recv() {
        if let gf_core::CommandPhase::Started {
            argv,
            read_only: false,
        } = e.phase
        {
            started.push(argv.join(" "));
        }
    }
    assert!(
        started.contains(&"gix branch logged".to_owned()),
        "{started:?}"
    );
}

// ------------------------------------------------------------------- fetch

/// An origin and a second remote, two identical clones of them, then
/// changes on origin: a new commit and tag, a new branch, a deleted branch
/// (pruned), a rewound branch (forced update) and a tag on an unreachable
/// commit (not auto-followed).
#[cfg(feature = "gix-fetch")]
fn fetch_twins() -> (TestRepo, TestRepo, TestRepo, TestRepo) {
    let origin = TestRepo::new();
    origin.write("a.txt", "a\n");
    origin.commit_all("A");
    origin.write("a.txt", "b\n");
    origin.commit_all("B");
    origin.git(&["tag", "-a", "v0", "-m", "zero"]);
    origin.git(&["branch", "keep"]);
    origin.git(&["branch", "doomed"]);
    let mirror = TestRepo::new();
    mirror.write("m.txt", "m\n");
    mirror.commit_all("M");

    let clone = || {
        let c = origin.clone_repo();
        c.git(&["remote", "add", "mirror", mirror.path().to_str().unwrap()]);
        c
    };
    let (a, b) = (clone(), clone());

    origin.write("a.txt", "c\n");
    origin.commit_all("C");
    origin.git(&["tag", "v1"]);
    origin.git(&["branch", "fresh"]);
    origin.git(&["branch", "-D", "doomed"]);
    origin.git(&["branch", "-f", "keep", "HEAD~2"]);
    origin.git(&["switch", "-q", "--detach"]);
    origin.write("o.txt", "o\n");
    origin.commit_all("orphan");
    origin.git(&["tag", "orphan"]);
    origin.git(&["switch", "-q", "main"]);
    mirror.write("m.txt", "m2\n");
    mirror.commit_all("M2");
    (origin, mirror, a, b)
}

#[cfg(feature = "gix-fetch")]
#[tokio::test]
async fn fetch_matches_the_git_cli() {
    for remote in [Some("origin"), None] {
        let (_origin, _mirror, a, b) = fetch_twins();
        let (ra, rb) = (open(&a).await, open(&b).await);
        let (cli, _) = ra.backends();
        let (_, gix) = rb.backends();
        gf_git::backend::SyncOps::fetch(cli, remote, gf_git::RunOptions::default())
            .await
            .unwrap();
        gf_git::backend::SyncOps::fetch(gix.unwrap(), remote, gf_git::RunOptions::default())
            .await
            .unwrap_or_else(|e| panic!("gix fetch {remote:?}: {e}"));
        let refs = |r: &TestRepo| r.git(&["for-each-ref", "--format=%(refname) %(objectname)"]);
        assert_eq!(
            refs(&b),
            refs(&a),
            "fetch {remote:?}: gix (left) vs git CLI (right)"
        );
        // Everything the refs point to arrived.
        b.git(&["fsck", "--connectivity-only", "--no-dangling"]);
    }
}

#[cfg(feature = "gix-fetch")]
#[tokio::test]
async fn fetch_routing_cancel_and_fallbacks() {
    let (_origin, _mirror, a, _b) = fetch_twins();
    let repo = open(&a).await;
    let (_, gix) = repo.backends();
    let gix = gix.unwrap();

    let token = gf_git::CancellationToken::new();
    token.cancel();
    let opts = gf_git::RunOptions {
        cancel: Some(token),
        ..gf_git::RunOptions::default()
    };
    assert!(matches!(
        gf_git::backend::SyncOps::fetch(gix, None, opts).await,
        Err(gf_core::GfError::Cancelled)
    ));
    assert!(matches!(
        gf_git::backend::SyncOps::fetch(
            gix,
            Some("https://example.com/x.git"),
            gf_git::RunOptions::default()
        )
        .await,
        Err(gf_core::GfError::Unsupported(_))
    ));

    // Off by default: the repository fetches with the CLI unless enabled.
    let repo = open(&a).await.with_gix_fetch(true);
    repo.fetch(None, gf_git::RunOptions::default())
        .await
        .unwrap();
    assert!(a.git(&["branch", "-r"]).contains("origin/fresh"));
}
