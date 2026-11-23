#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};

use common::{open, repo_with_merge, runner};
use gf_core::GfError;
use gf_core::model::{ChangeKind, ConflictKind, FileChange, RefKind, StatusEntry};
use gf_git::{DiffOptions, DiffTarget, LogOrder, LogQuery, Repository};
use gf_test_support::TestRepo;

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap()
}

// ---------------------------------------------------------------- open

#[tokio::test]
async fn opens_from_root_and_subdirectory() {
    let repo = TestRepo::new();
    repo.write("a/b/c.txt", "x\n");
    repo.commit_all("init");

    let from_root = open(&repo).await;
    let from_sub = Repository::open(repo.path().join("a/b"), runner(&repo))
        .await
        .unwrap();

    let expected = canonical(repo.path());
    assert_eq!(
        from_root.handle().workdir.as_deref().map(canonical),
        Some(expected.clone())
    );
    assert_eq!(
        from_sub.handle().workdir.as_deref().map(canonical),
        Some(expected)
    );
    assert!(from_root.handle().git_dir.ends_with(".git"));
}

#[tokio::test]
async fn commit_file_paths_use_the_requested_tree_and_ignore_worktree_files() {
    let repo = TestRepo::new();
    repo.write("shared.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "feature"]);
    // Windows file names cannot hold a newline.
    let odd_name = if cfg!(windows) {
        "odd name.txt"
    } else {
        "line\nbreak.txt"
    };
    repo.write(odd_name, "newline path\n");
    let feature = repo.commit_all("feature paths");

    repo.git(&["switch", "-q", "main"]);
    repo.git(&["rm", "-q", "shared.txt"]);
    repo.write("main-only.txt", "main\n");
    let main = repo.commit_all("main paths");
    repo.write("untracked.txt", "not committed\n");

    let git = open(&repo).await;
    let feature_oid = feature.parse().unwrap();
    let feature_paths = git.commit_file_paths(&feature_oid).await.unwrap();
    assert!(feature_paths.contains(&PathBuf::from("shared.txt")));
    assert!(feature_paths.contains(&PathBuf::from(odd_name)));
    assert!(!feature_paths.contains(&PathBuf::from("main-only.txt")));
    assert!(!feature_paths.contains(&PathBuf::from("untracked.txt")));

    let main_paths = git.commit_file_paths(&main.parse().unwrap()).await.unwrap();
    assert!(main_paths.contains(&PathBuf::from("main-only.txt")));
    assert!(!main_paths.contains(&PathBuf::from("shared.txt")));
    assert!(!main_paths.contains(&PathBuf::from(odd_name)));
}

#[tokio::test]
async fn commit_file_paths_handle_empty_trees_and_invalid_commits() {
    let repo = TestRepo::new();
    let empty = repo.commit_all("empty");
    let git = open(&repo).await;

    assert!(
        git.commit_file_paths(&empty.parse().unwrap())
            .await
            .unwrap()
            .is_empty()
    );
    let missing = "0000000000000000000000000000000000000000".parse().unwrap();
    assert!(git.commit_file_paths(&missing).await.is_err());
}

#[tokio::test]
async fn file_history_follows_renames_but_not_copies() {
    let repo = TestRepo::new();
    repo.write("CLAUDE.md", "guidance\nline two\nline three\n");
    repo.commit_all("add claude");
    repo.write("CLAUDE.md", "guidance\nline two changed\nline three\n");
    repo.commit_all("edit claude");
    repo.write("AGENTS.md", "guidance\nline two changed\nline three\n");
    let copy = repo.commit_all("copy to agents");
    repo.write("old.txt", "renamed content\nmore\nlines\n");
    repo.commit_all("add old");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.commit_all("rename");
    repo.write("new.txt", "renamed content\nmore\nlines\nedit\n");
    repo.commit_all("edit new");

    let git = open(&repo).await;
    let history = |path: &str| {
        let q = LogQuery {
            paths: vec![path.into()],
            follow: true,
            ..LogQuery::default()
        };
        let git = &git;
        async move {
            git.log(&q)
                .await
                .unwrap()
                .into_iter()
                .map(|c| c.summary)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(history("AGENTS.md").await, ["copy to agents"], "{copy}");
    assert_eq!(
        history("new.txt").await,
        ["edit new", "rename", "add old"],
        "renames keep their history"
    );
}

#[tokio::test]
async fn opens_bare_repository() {
    let bare = TestRepo::new_bare();
    let repo = open(&bare).await;
    assert!(repo.handle().is_bare());
}

#[tokio::test]
async fn rejects_non_repository() {
    let dir = tempfile_dir();
    let repo = TestRepo::new(); // only for its isolated env
    let err = Repository::open(dir.path(), runner(&repo))
        .await
        .unwrap_err();
    assert!(matches!(err, GfError::NotARepository(_)), "{err}");
}

fn tempfile_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

// ---------------------------------------------------------------- status

#[tokio::test]
async fn status_reports_every_kind_of_change() {
    let repo = TestRepo::new();
    repo.write("modified.txt", "1\n");
    repo.write("staged.txt", "1\n");
    repo.write("to-rename.txt", "rename me please\n");
    repo.commit_all("init");

    repo.write("modified.txt", "2\n");
    repo.write("staged.txt", "2\n");
    repo.git(&["add", "staged.txt"]);
    repo.git(&["mv", "to-rename.txt", "renamed.txt"]);
    repo.write("new dir/untracked file.txt", "u\n");

    let status = open(&repo).await.status().await.unwrap();
    assert_eq!(status.head.branch.as_deref(), Some("main"));
    assert!(status.head.oid.is_some());

    let find = |name: &str| {
        status
            .entries
            .iter()
            .find(|e| e.path() == Path::new(name))
            .unwrap_or_else(|| panic!("{name} missing from {:?}", status.entries))
    };
    assert!(matches!(
        find("modified.txt"),
        StatusEntry::Changed {
            index: ChangeKind::Unmodified,
            worktree: ChangeKind::Modified,
            ..
        }
    ));
    assert!(find("staged.txt").is_staged());
    assert!(matches!(
        find("renamed.txt"),
        StatusEntry::Changed { index: ChangeKind::Renamed, orig_path: Some(p), .. } if p == Path::new("to-rename.txt")
    ));
    assert!(matches!(
        find("new dir/untracked file.txt"),
        StatusEntry::Untracked { .. }
    ));
}

#[tokio::test]
async fn status_reports_conflicts() {
    let repo = TestRepo::new();
    repo.write("f.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["switch", "-q", "-c", "other"]);
    repo.write("f.txt", "theirs\n");
    repo.commit_all("theirs");
    repo.git(&["switch", "-q", "main"]);
    repo.write("f.txt", "ours\n");
    repo.commit_all("ours");
    assert!(!repo.git_output(&["merge", "other"]).status.success());

    let status = open(&repo).await.status().await.unwrap();
    assert_eq!(
        status.entries,
        vec![StatusEntry::Conflicted {
            path: "f.txt".into(),
            kind: ConflictKind::BothModified
        }]
    );
}

#[tokio::test]
async fn status_on_unborn_branch_and_upstream_counts() {
    let empty = TestRepo::new();
    let status = open(&empty).await.status().await.unwrap();
    assert_eq!(status.head.oid, None);
    assert_eq!(status.head.branch.as_deref(), Some("main"));

    let origin = TestRepo::new();
    origin.write("a", "1\n");
    origin.commit_all("one");
    let clone = origin.clone_repo();
    clone.write("b", "2\n");
    clone.commit_all("local");
    origin.write("c", "3\n");
    origin.commit_all("remote");
    clone.git(&["fetch", "-q"]);

    let head = open(&clone).await.status().await.unwrap().head;
    assert_eq!(head.upstream.as_deref(), Some("origin/main"));
    assert_eq!((head.ahead, head.behind), (1, 1));
}

// ---------------------------------------------------------------- log

#[tokio::test]
async fn log_parses_commits_and_merges() {
    let repo = repo_with_merge();
    let commits = open(&repo)
        .await
        .log_cli(&LogQuery::default())
        .await
        .unwrap();

    let summaries: Vec<_> = commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries.len(), 5);
    assert_eq!(summaries[0], "M: merge feature");
    assert!(commits[0].is_merge());
    assert_eq!(
        commits.last().map(|c| c.summary.as_str()),
        Some("A: initial")
    );
    assert!(commits.last().unwrap().is_root());

    let b = commits
        .iter()
        .find(|c| c.summary.starts_with("B:"))
        .unwrap();
    assert_eq!(b.body, "With a body line.");
    assert_eq!(b.author.name, "Test Author");
    assert_eq!(b.committer.email, "committer@example.com");
    assert_eq!(b.author.time.offset_minutes, 0);
}

#[cfg(feature = "gix")]
#[tokio::test]
async fn gix_fast_path_matches_cli() {
    let repo = repo_with_merge();
    let r = open(&repo).await;

    for query in [
        LogQuery::default(),
        LogQuery::head(2),
        LogQuery {
            skip: 1,
            max_count: Some(3),
            ..LogQuery::default()
        },
        LogQuery {
            revisions: vec!["feature".into()],
            ..LogQuery::default()
        },
    ] {
        assert!(query.is_plain_walk());
        let fast = r.log_fast(&query).await.expect("gix path applies").unwrap();
        let cli = r.log_cli(&query).await.unwrap();
        assert_eq!(fast, cli, "query {query:?}");
    }
}

#[tokio::test]
async fn log_filters_use_cli() {
    let repo = repo_with_merge();
    let r = open(&repo).await;

    let by_path = r
        .log(&LogQuery {
            paths: vec!["src/feature.rs".into()],
            ..LogQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(by_path.len(), 1);
    assert_eq!(by_path[0].summary, "F: feature work");

    let grep = r
        .log(&LogQuery {
            grep: Some("changelog".into()),
            ..LogQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(grep.len(), 1);

    let first_parent = r
        .log(&LogQuery {
            first_parent: true,
            order: LogOrder::Topo,
            ..LogQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(
        first_parent.len(),
        4,
        "F is only reachable via the merge's second parent"
    );

    let range = r
        .log(&LogQuery {
            revisions: vec!["main..feature".into()],
            ..LogQuery::default()
        })
        .await
        .unwrap();
    assert!(range.is_empty(), "feature is fully merged");
}

#[tokio::test]
async fn log_on_unborn_branch_is_empty() {
    let repo = TestRepo::new();
    let r = open(&repo).await;
    assert!(r.log(&LogQuery::default()).await.unwrap().is_empty());
    assert!(r.log_cli(&LogQuery::default()).await.unwrap().is_empty());
    assert!(!r.head_exists().await.unwrap());
}

// ---------------------------------------------------------------- refs

#[tokio::test]
async fn branches_tags_and_remotes() {
    let origin = repo_with_merge();
    origin.git(&["tag", "light"]);
    origin.git(&["tag", "-a", "v1.0", "-m", "Release 1.0\n\nNotes", "HEAD~1"]);
    let clone = origin.clone_repo();
    clone.git(&["remote", "add", "fork", "https://example.com/fork.git"]);
    clone.git(&[
        "remote",
        "set-url",
        "--push",
        "fork",
        "ssh://example.com/fork.git",
    ]);

    let r = open(&clone).await;
    let branches = r.branches().await.unwrap();
    let main = branches
        .iter()
        .find(|b| b.name.as_str() == "refs/heads/main")
        .unwrap();
    assert!(main.is_head);
    assert_eq!(
        main.upstream.as_ref().map(|u| u.name.short()),
        Some("origin/main")
    );
    assert!(
        branches
            .iter()
            .any(|b| b.name.kind() == RefKind::RemoteBranch && b.name.short() == "origin/feature")
    );
    assert!(!branches.iter().any(|b| b.name.short() == "origin/HEAD"));

    let tags = r.tags().await.unwrap();
    let v1 = tags.iter().find(|t| t.name.short() == "v1.0").unwrap();
    assert_eq!(v1.message.as_deref(), Some("Release 1.0\n\nNotes"));
    assert_eq!(v1.target.as_str(), clone.git(&["rev-parse", "HEAD~1"]));
    let light = tags.iter().find(|t| t.name.short() == "light").unwrap();
    assert_eq!(light.message, None);

    let remotes = r.remotes().await.unwrap();
    assert_eq!(remotes.len(), 2);
    let fork = remotes.iter().find(|x| x.name == "fork").unwrap();
    assert_eq!(fork.effective_push_url(), "ssh://example.com/fork.git");
}

// ---------------------------------------------------------------- diff

#[tokio::test]
async fn diff_worktree_staged_and_commit() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\ntwo\nthree\n");
    repo.write(
        "move-me.txt",
        "content that is long enough to detect a rename\n",
    );
    let root = repo.commit_all("init");

    repo.write("a.txt", "one\n2\nthree\n");
    repo.write("new.txt", "fresh\n");
    repo.git(&["add", "new.txt"]);
    repo.git(&["mv", "move-me.txt", "moved.txt"]);
    std::fs::write(repo.path().join("logo.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
    repo.git(&["add", "logo.bin"]);

    let r = open(&repo).await;
    let opts = DiffOptions::default();

    let wt = r.diff(&DiffTarget::WorkTree, &opts).await.unwrap();
    assert_eq!(wt.len(), 1);
    assert_eq!(wt[0].change, FileChange::Modified);
    assert_eq!(wt[0].line_stats(), (1, 1));

    let staged = r.diff(&DiffTarget::Staged, &opts).await.unwrap();
    let path_of = |f: &gf_core::model::FileDiff| f.display_path().cloned().unwrap();
    let new = staged
        .iter()
        .find(|f| path_of(f) == Path::new("new.txt"))
        .unwrap();
    assert_eq!(new.change, FileChange::Added);
    let moved = staged
        .iter()
        .find(|f| path_of(f) == Path::new("moved.txt"))
        .unwrap();
    assert_eq!(moved.change, FileChange::Renamed { similarity: 100 });
    assert_eq!(moved.old_path.as_deref(), Some(Path::new("move-me.txt")));
    let bin = staged
        .iter()
        .find(|f| path_of(f) == Path::new("logo.bin"))
        .unwrap();
    assert!(bin.is_binary);

    let first = r
        .diff(&DiffTarget::Commit(root.parse().unwrap()), &opts)
        .await
        .unwrap();
    assert_eq!(first.len(), 2, "root commit diffs against the empty tree");
    assert!(first.iter().all(|f| f.change == FileChange::Added));

    let limited = r
        .diff(
            &DiffTarget::Staged,
            &DiffOptions {
                paths: vec!["new.txt".into()],
                ..opts
            },
        )
        .await
        .unwrap();
    assert_eq!(limited.len(), 1);
}

#[tokio::test]
async fn diff_of_merge_commit_uses_first_parent() {
    let repo = repo_with_merge();
    let r = open(&repo).await;
    let head = r.log(&LogQuery::head(1)).await.unwrap().remove(0);
    let files = r
        .diff(&DiffTarget::Commit(head.id), &DiffOptions::default())
        .await
        .unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].new_path.as_deref(),
        Some(Path::new("src/feature.rs"))
    );
}

#[tokio::test]
async fn diffs_past_the_limit_are_refused_without_reading_them() {
    let repo = TestRepo::new();
    repo.write("small.txt", "a\n");
    repo.commit_all("init");
    // Lines of text, so git prints a text diff (one byte past the limit).
    let line = "x".repeat(99) + "\n";
    let big = line.repeat(gf_git::DIFF_LIMIT / line.len() + 1);
    repo.write("big.txt", &big);
    let r = open(&repo).await;
    let opts = DiffOptions::default();

    let untracked = r
        .diff(&DiffTarget::Untracked("big.txt".into()), &opts)
        .await;
    assert!(
        matches!(untracked, Err(GfError::DiffTooLarge { limit }) if limit == gf_git::DIFF_LIMIT),
        "{untracked:?}"
    );

    let id = repo.commit_all("big");
    let commit = r
        .diff(&DiffTarget::Commit(id.parse().unwrap()), &opts)
        .await;
    assert!(
        matches!(commit, Err(GfError::DiffTooLarge { .. })),
        "{commit:?}"
    );
    assert!(
        GfError::DiffTooLarge {
            limit: gf_git::DIFF_LIMIT
        }
        .to_string()
        .contains("32 MiB")
    );

    // A diff under the limit still loads.
    repo.write("small.txt", "b\n");
    let files = r.diff(&DiffTarget::WorkTree, &opts).await.unwrap();
    assert_eq!(files.len(), 1);
}

// ------------------------------------------------------- log position

/// Branches, a merge, a branch merged nowhere and a tag on it.
fn tangled_repo() -> TestRepo {
    let repo = common::repo_with_merge();
    repo.git(&["switch", "-q", "-c", "side", "HEAD~2"]);
    for i in 0..5 {
        repo.write("side.txt", &format!("{i}\n"));
        repo.commit_all(&format!("S{i}: side work"));
    }
    repo.git(&["tag", "side-tag"]);
    repo.git(&["switch", "-q", "main"]);
    for i in 0..3 {
        repo.write("main.txt", &format!("{i}\n"));
        repo.commit_all(&format!("N{i}: more main"));
    }
    repo
}

#[tokio::test]
async fn log_position_matches_the_row_in_the_history() {
    use gf_git::{LogOrder, LogQuery};
    let repo = tangled_repo();
    let r = open(&repo).await;
    for (all_refs, revisions) in [
        (true, vec![]),
        (false, vec![]),
        (false, vec!["side".to_owned()]),
        (false, vec!["feature".to_owned()]),
    ] {
        let query = LogQuery {
            revisions,
            all_refs,
            order: LogOrder::Topo,
            ..LogQuery::default()
        };
        let rows = r.log(&query).await.unwrap();
        assert!(!rows.is_empty());
        for (i, c) in rows.iter().enumerate() {
            assert_eq!(
                r.log_position(&query, &c.id).await.unwrap(),
                Some(i),
                "{} in {query:?}",
                c.summary
            );
        }
    }
}

#[tokio::test]
async fn log_position_is_none_outside_the_history() {
    use gf_git::{LogOrder, LogQuery};
    let repo = tangled_repo();
    let r = open(&repo).await;
    let side: gf_core::model::Oid = repo.git(&["rev-parse", "side"]).trim().parse().unwrap();
    let head_only = LogQuery {
        order: LogOrder::Topo,
        ..LogQuery::default()
    };
    assert_eq!(
        r.log_position(&head_only, &side).await.unwrap(),
        None,
        "not reachable from HEAD"
    );
    let all = LogQuery {
        all_refs: true,
        ..head_only.clone()
    };
    assert!(r.log_position(&all, &side).await.unwrap().is_some());
    let unknown: gf_core::model::Oid = "1".repeat(40).parse().unwrap();
    assert_eq!(r.log_position(&all, &unknown).await.unwrap(), None);
}

#[tokio::test]
async fn file_preview_limits_each_version_before_reading_its_contents() {
    use gf_git::{FILE_PREVIEW_LIMIT, FileVersion};

    let repo = TestRepo::new();
    let path = Path::new("preview.txt");
    let limit = "a".repeat(FILE_PREVIEW_LIMIT);
    repo.write("preview.txt", &limit);
    repo.commit_all("exact limit");

    let r = open(&repo).await;
    let versions = [
        FileVersion::WorkTree,
        FileVersion::Index,
        FileVersion::Revision("HEAD".into()),
    ];
    for version in &versions {
        assert_eq!(r.file_bytes(path, version).await.unwrap(), limit.as_bytes());
    }

    // Worktree and staged changes are independent of the committed file.
    repo.write("preview.txt", &format!("{limit}b"));
    assert!(matches!(
        r.file_bytes(path, &FileVersion::WorkTree).await,
        Err(GfError::FilePreviewTooLarge)
    ));
    assert_eq!(
        r.file_bytes(path, &FileVersion::Index).await.unwrap().len(),
        FILE_PREVIEW_LIMIT
    );
    repo.git(&["add", "--", "preview.txt"]);
    assert!(matches!(
        r.file_bytes(path, &FileVersion::Index).await,
        Err(GfError::FilePreviewTooLarge)
    ));
    repo.commit_all("over limit");
    assert!(matches!(
        r.file_bytes(path, &FileVersion::Revision("HEAD".into()))
            .await,
        Err(GfError::FilePreviewTooLarge)
    ));
    assert_eq!(
        r.file_bytes(path, &FileVersion::Revision("HEAD^".into()))
            .await
            .unwrap(),
        limit.as_bytes()
    );
}
