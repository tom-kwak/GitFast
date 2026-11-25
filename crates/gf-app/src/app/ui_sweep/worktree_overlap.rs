//! Shared-file badges with real linked worktrees and background status reads.

use std::path::Path;

use super::{Message, Ui, two_trees, worktrees};

fn card<'a>(ui: &'a Ui, path: &Path) -> &'a worktrees::Card {
    ui.app.tabs[0]
        .worktrees
        .cards()
        .iter()
        .find(|c| c.tree.path == path)
        .expect("worktree card")
}

fn assert_shared_file(ui: &Ui, a: &Path, b: &Path, path: &str) {
    for (tree, other) in [(a, "fix"), (b, "feat")] {
        let overlap = card(ui, tree).overlap.as_ref().expect("shared file");
        assert_eq!(overlap.files, 1);
        assert!(overlap.tip.contains(&format!("{other}: {path}\n")));
    }
}

#[tokio::test]
async fn untracked_worktree_files_match_by_path_before_and_after_commit() {
    let (mut ui, a, b) = two_trees("feat", "fix").await;
    for (tree, name) in [(&a, "left.txt"), (&b, "right.txt")] {
        std::fs::create_dir(tree.join("pkg")).expect("folder");
        std::fs::write(tree.join("pkg").join(name), "change\n").expect("file");
    }
    let _ = ui.send(Message::WorktreePoll).await;
    assert!(card(&ui, &a).overlap.is_none());
    assert!(card(&ui, &b).overlap.is_none());

    std::fs::write(b.join("pkg/left.txt"), "other change\n").expect("shared file");
    let _ = ui.send(Message::WorktreePoll).await;
    assert_shared_file(&ui, &a, &b, "pkg/left.txt");

    let dir = b.to_str().expect("worktree path");
    ui.fixture.repo.git(&["-C", dir, "add", "pkg"]);
    ui.fixture
        .repo
        .git(&["-C", dir, "commit", "-qm", "add pkg files"]);
    let _ = ui.send(Message::WorktreePoll).await;
    assert_shared_file(&ui, &a, &b, "pkg/left.txt");

    // This config routes worktree status through the CLI.
    ui.fixture
        .repo
        .git(&["config", "diff.ignoreSubmodules", "none"]);
    let _ = ui.send(Message::WorktreePoll).await;
    assert_shared_file(&ui, &a, &b, "pkg/left.txt");
    let _ = std::fs::remove_dir_all(a.parent().expect("root"));
}

#[tokio::test]
async fn a_worktree_read_error_clears_both_shared_badges() {
    let (mut ui, a, b) = two_trees("feat", "fix").await;
    for tree in [&a, &b] {
        std::fs::write(tree.join("shared.txt"), "shared\n").expect("file");
    }
    let _ = ui.send(Message::WorktreePoll).await;
    assert_shared_file(&ui, &a, &b, "shared.txt");

    let moved = tempfile::tempdir().expect("temporary folder");
    let away = moved.path().join("feat");
    std::fs::rename(&a, &away).expect("move worktree away");
    let _ = ui.send(Message::WorktreePoll).await;
    assert!(card(&ui, &a).error.is_some());
    assert!(card(&ui, &a).status.is_none());
    assert!(card(&ui, &a).overlap.is_none());
    assert!(card(&ui, &b).overlap.is_none());

    std::fs::rename(away, &a).expect("restore worktree");
    let _ = ui.send(Message::WorktreePoll).await;
    assert!(card(&ui, &a).error.is_none());
    assert_shared_file(&ui, &a, &b, "shared.txt");
    let _ = std::fs::remove_dir_all(a.parent().expect("root"));
}

#[tokio::test]
async fn shared_files_are_detected_in_worktrees_with_more_than_two_thousand_changes() {
    let (mut ui, a, b) = two_trees("feat", "fix").await;
    std::fs::write(a.join("shared.txt"), "feat\n").expect("file");
    std::fs::write(b.join("shared.txt"), "fix\n").expect("file");
    for i in 0..2000 {
        std::fs::write(b.join(format!("fix-{i}.txt")), "fix\n").expect("file");
    }
    let dir = b.to_str().expect("worktree path");
    ui.fixture.repo.git(&["-C", dir, "add", "-A"]);
    ui.fixture
        .repo
        .git(&["-C", dir, "commit", "-qm", "many files"]);
    let _ = ui.send(Message::WorktreePoll).await;
    assert_shared_file(&ui, &a, &b, "shared.txt");
    let _ = std::fs::remove_dir_all(a.parent().expect("root"));
}
