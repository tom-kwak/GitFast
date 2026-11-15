#![allow(dead_code, clippy::unwrap_used)]

use gf_git::{GitRunner, Repository};
use gf_test_support::TestRepo;

/// A runner isolated the same way as the fixture's own git calls.
pub fn runner(repo: &TestRepo) -> GitRunner {
    GitRunner::new().with_cleared_env().with_env(repo.env())
}

pub async fn open(repo: &TestRepo) -> Repository {
    Repository::open(repo.path(), runner(repo)).await.unwrap()
}

/// main: A ─ B ─ M (merge of feature)
///             └ F ┘
pub fn repo_with_merge() -> TestRepo {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.commit_all("A: initial");
    repo.write("src/lib.rs", "fn a() {}\n");
    repo.commit_all("B: add lib\n\nWith a body line.");
    repo.git(&["switch", "-q", "-c", "feature"]);
    repo.write("src/feature.rs", "fn f() {}\n");
    repo.commit_all("F: feature work");
    repo.git(&["switch", "-q", "main"]);
    repo.write("CHANGELOG.md", "- b\n");
    repo.commit_all("C: changelog");
    repo.git(&[
        "merge",
        "-q",
        "--no-ff",
        "-m",
        "M: merge feature",
        "feature",
    ]);
    repo
}
