//! Throwaway git repositories for tests.
//!
//! Every git invocation ignores the developer's global/system config and uses
//! fixed author/committer identities and dates, so commit ids are reproducible
//! and tests behave the same locally and in CI.
//!
//! ```no_run
//! use gf_test_support::TestRepo;
//!
//! let repo = TestRepo::new();
//! repo.write("README.md", "hello\n");
//! repo.commit_all("initial");
//! repo.git(&["switch", "-c", "feature"]);
//! ```

// Test helpers fail fast: a broken fixture should abort the test immediately.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use tempfile::TempDir;

/// A git repository in a temporary directory, deleted on drop.
#[derive(Debug)]
pub struct TestRepo {
    dir: TempDir,
    /// Increments per commit so each commit gets a distinct, fixed date.
    clock: AtomicU32,
}

/// Base timestamp for fixture commits: 2024-01-01T00:00:00Z.
const BASE_EPOCH: u32 = 1_704_067_200;

impl TestRepo {
    /// `git init` with `main` as the initial branch.
    pub fn new() -> Self {
        let repo = Self::empty_dir();
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    /// `git init --bare`, useful as a fake remote for push/fetch tests.
    pub fn new_bare() -> Self {
        let repo = Self::empty_dir();
        repo.git(&["init", "-q", "--bare", "-b", "main"]);
        repo
    }

    fn empty_dir() -> Self {
        Self {
            dir: tempfile::Builder::new()
                .prefix("gf-test-")
                .tempdir()
                .unwrap(),
            clock: AtomicU32::new(0),
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// A `git` command preconfigured to run in this repository in isolation.
    pub fn command(&self) -> Command {
        isolated_git(self.path(), self.clock.load(Ordering::SeqCst))
    }

    /// Runs git and returns stdout (trimmed). Panics with stderr on failure.
    pub fn git(&self, args: &[&str]) -> String {
        let out = self.command().args(args).output().unwrap();
        assert_success(args, &out);
        String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
    }

    /// Runs git and returns the raw output without checking the exit code,
    /// for tests that expect failures (e.g. merge conflicts).
    pub fn git_output(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    /// Writes a file relative to the repository root, creating parent dirs.
    pub fn write(&self, rel: impl AsRef<Path>, contents: &str) -> PathBuf {
        let path = self.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// Stages everything and commits. Returns the new commit id.
    pub fn commit_all(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.clock.fetch_add(1, Ordering::SeqCst);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        self.head()
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    /// Creates a clone of this repository next to it (in its own temp dir).
    pub fn clone_repo(&self) -> TestRepo {
        let clone = Self::empty_dir();
        let out = isolated_git(clone.path(), 0)
            .args(["clone", "-q"])
            .arg(self.path())
            .arg(".")
            .output()
            .unwrap();
        assert_success(&["clone"], &out);
        clone
    }
}

impl Default for TestRepo {
    fn default() -> Self {
        Self::new()
    }
}

fn isolated_git(cwd: &Path, tick: u32) -> Command {
    let date = format!("@{} +0000", BASE_EPOCH + tick * 60);
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_device())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_NAME", "Test Committer")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_COMMITTER_DATE", &date)
        .env("LC_ALL", "C");
    cmd
}

fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

fn assert_success(args: &[&str], out: &Output) {
    assert!(
        out.status.success(),
        "git {} failed ({}):\n{}",
        args.join(" "),
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commits_are_reproducible() {
        let make = || {
            let repo = TestRepo::new();
            repo.write("a.txt", "a\n");
            repo.commit_all("first")
        };
        assert_eq!(make(), make());
    }

    #[test]
    fn branches_and_clones() {
        let repo = TestRepo::new();
        repo.write("src/lib.rs", "// lib\n");
        let first = repo.commit_all("first");
        repo.git(&["switch", "-q", "-c", "feature"]);
        repo.write("src/lib.rs", "// changed\n");
        repo.commit_all("second");

        assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "2");
        assert_eq!(repo.git(&["rev-parse", "main"]), first);

        let clone = repo.clone_repo();
        assert_eq!(clone.git(&["rev-parse", "origin/feature"]), repo.head());
    }

    #[test]
    fn expected_failures_are_observable() {
        let repo = TestRepo::new();
        let out = repo.git_output(&["rev-parse", "does-not-exist"]);
        assert!(!out.status.success());
    }
}
