//! Linked working trees (`git worktree`): the list, what each one is doing
//! right now ([`WorktreeStatus`]) and add / remove / lock / prune.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gf_core::model::{Oid, StatusEntry, StatusReport};
use gf_core::{GfError, Result};

use crate::backend::{CliBackend, WorktreeOps};
use crate::cli::parse::status::parse_status;
use crate::{DIFF_LIMIT, RunOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: Option<Oid>,
    /// Short branch name; `None` when detached.
    pub branch: Option<String>,
    pub bare: bool,
    pub locked: bool,
    /// Its directory is gone; `prune` cleans it up.
    pub prunable: bool,
}

/// Parses `git worktree list --porcelain` (blank-line separated records).
pub fn parse_worktrees(out: &[u8]) -> Result<Vec<Worktree>> {
    let text = String::from_utf8_lossy(out);
    let mut list = Vec::new();
    for record in text.split("\n\n").filter(|r| !r.trim().is_empty()) {
        let mut wt = Worktree {
            path: PathBuf::new(),
            head: None,
            branch: None,
            bare: false,
            locked: false,
            prunable: false,
        };
        for line in record.lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "worktree" => wt.path = PathBuf::from(value),
                "HEAD" => wt.head = value.parse().ok(),
                "branch" => wt.branch = Some(value.trim_start_matches("refs/heads/").to_owned()),
                "bare" => wt.bare = true,
                "locked" => wt.locked = true,
                "prunable" => wt.prunable = true,
                _ => {}
            }
        }
        if wt.path.as_os_str().is_empty() {
            return Err(GfError::parse("worktree list", record.to_owned()));
        }
        list.push(wt);
    }
    Ok(list)
}

/// What one working tree is doing: the summary on its card in the
/// worktrees view. Read in that tree's folder, so it sees its own HEAD,
/// index and files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeStatus {
    /// Files with staged changes.
    pub staged: usize,
    /// Tracked files with unstaged changes.
    pub unstaged: usize,
    /// Untracked files.
    pub untracked: usize,
    pub conflicts: usize,
    /// Upstream short name and how far HEAD is ahead of / behind it.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// The branch the work goes into: the one this branch was made from
    /// (see [`CliBackend::base_of`]), when there is one and HEAD is not on
    /// the default base itself.
    pub base: Option<String>,
    /// Nothing records where the branch came from: [`Self::base`] is the
    /// nearest of the usual branches, a guess the user can correct.
    pub base_guessed: bool,
    /// Commits on HEAD that the base does not have: the work done here.
    /// Merge commits are left out: bringing the base in is not work.
    pub ahead_of_base: u32,
    /// Commits on the base that HEAD does not have.
    pub behind_base: u32,
    /// Where HEAD left the base: comparing from here shows only the work
    /// done in this tree.
    pub merge_base: Option<Oid>,
    /// A commit the branch made itself (see [`made_here`]) is in the base:
    /// with nothing ahead, its work was merged. A branch that only took in
    /// the base (fast-forwards, rebases that carried nothing, resets) or
    /// whose commits were thrown away made nothing that got there.
    pub work_in_base: bool,
    /// Its commits are not in the base, but every file they change is the
    /// same there as here: merged by squash, rebase or cherry-pick.
    pub squashed: bool,
    pub last_commit: Option<LastCommit>,
    /// Newest modification time among the changed files: whether someone
    /// (an agent) is editing in this tree right now.
    pub last_activity: Option<SystemTime>,
    /// Sorted paths changed by pending commits (both paths of a rename)
    /// or by uncommitted changes, including individual untracked files.
    /// `None` when the committed paths could not be read.
    pub touched: Option<Vec<PathBuf>>,
}

/// Joins pending commits and uncommitted changes into sorted, unique paths.
pub(crate) fn touched(report: &StatusReport, mut committed: Vec<PathBuf>) -> Vec<PathBuf> {
    for e in &report.entries {
        match e {
            StatusEntry::Ignored { .. } => {}
            StatusEntry::Changed {
                path, orig_path, ..
            } => {
                committed.push(path.clone());
                committed.extend(orig_path.clone());
            }
            other => committed.push(other.path().clone()),
        }
    }
    committed.sort_unstable();
    committed.dedup();
    committed
}

impl WorktreeStatus {
    /// Uncommitted entries of any kind.
    pub fn changed(&self) -> usize {
        self.staged + self.unstaged + self.untracked + self.conflicts
    }

    pub fn is_clean(&self) -> bool {
        self.changed() == 0
    }

    /// The branch had work of its own and all of it is in the base (merged,
    /// fast-forwarded, or squashed into it): the tree can go.
    pub fn merged(&self) -> bool {
        self.base.is_some() && ((self.ahead_of_base == 0 && self.work_in_base) || self.squashed)
    }

    /// Nothing here the base lacks: no commit of its own outside it, or
    /// only ones whose changes it has. Removing the branch loses nothing.
    pub fn all_in_base(&self) -> bool {
        self.ahead_of_base == 0 || self.squashed
    }
}

/// The commit HEAD points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastCommit {
    pub id: Oid,
    pub subject: String,
    /// Committer time, seconds since the epoch.
    pub time: i64,
}

/// Counts of a status report, without the ignored entries.
pub fn status_counts(report: &StatusReport) -> WorktreeStatus {
    let mut s = WorktreeStatus {
        upstream: report.head.upstream.clone(),
        ahead: report.head.ahead,
        behind: report.head.behind,
        ..WorktreeStatus::default()
    };
    for e in &report.entries {
        match e {
            StatusEntry::Changed { .. } => {
                s.staged += usize::from(e.is_staged());
                s.unstaged += usize::from(e.has_unstaged_changes());
            }
            StatusEntry::Conflicted { .. } => s.conflicts += 1,
            StatusEntry::Untracked { .. } => s.untracked += 1,
            StatusEntry::Ignored { .. } => {}
        }
    }
    s
}

/// Parses `git log -1 --format=%H%x00%ct%x00%s`; `None` on an unborn branch.
pub fn parse_last_commit(out: &[u8]) -> Option<LastCommit> {
    let text = String::from_utf8_lossy(out);
    let mut parts = text.trim_end_matches('\n').splitn(3, '\0');
    Some(LastCommit {
        id: parts.next()?.parse().ok()?,
        time: parts.next()?.parse().ok()?,
        subject: parts.next().unwrap_or_default().to_owned(),
    })
}

/// Parses `git reflog show --format=%H%x00%gs`: `(new id, message)`,
/// newest first, without entries that have no message.
pub fn parse_reflog(out: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(out)
        .lines()
        .filter_map(|l| {
            let (id, message) = l.split_once('\0')?;
            (!message.is_empty()).then(|| (id.to_owned(), message.to_owned()))
        })
        .collect()
}

/// [`WorktreeStatus::squashed`]: none of the files the branch's commits
/// change (`committed`) is among those that differ between the base and
/// HEAD.
pub(crate) fn squashed(committed: &[PathBuf], differ: &[PathBuf]) -> bool {
    let differ: std::collections::HashSet<&PathBuf> = differ.iter().collect();
    !committed.is_empty() && !committed.iter().any(|p| differ.contains(p))
}

/// Parses `git rev-list --left-right --count A...B`: (only in A, only in B).
pub fn parse_left_right(out: &[u8]) -> Option<(u32, u32)> {
    let text = String::from_utf8_lossy(out);
    let mut n = text.split_whitespace().map(str::parse);
    Some((n.next()?.ok()?, n.next()?.ok()?))
}

/// How many of the newest commits [`made_here`] lists are looked for in the
/// base: enough for any branch a tree is made for.
pub(crate) const MADE_HERE_CHECKED: usize = 64;

/// Commits a branch made itself, newest first, from its reflog (`(new id,
/// message)`, newest first): commits (with amends, merges it concluded),
/// cherry-picks, reverts, and the tip of a rebase that carried commits
/// onto its target. Fast-forwards, merges from the base, resets and
/// rebases that carried nothing only took in what was already there.
pub(crate) fn made_here(reflog: &[(String, String)]) -> impl Iterator<Item = &str> {
    reflog.iter().filter_map(|(to, message)| {
        let own = ["commit", "cherry-pick", "revert"]
            .iter()
            .any(|p| message.starts_with(p))
            || (message.starts_with("rebase")
                && message.contains("(finish)")
                && message
                    .rsplit_once(" onto ")
                    .is_some_and(|(_, onto)| onto.trim() != to));
        own.then_some(to.as_str())
    })
}

/// Branches work usually goes into, tried after the default base when
/// nothing records where a branch came from.
pub(crate) const USUAL_BASES: [&str; 6] =
    ["main", "master", "develop", "development", "dev", "trunk"];

/// The git config key gf keeps a branch's base under (`branch.<b>.gf-base`);
/// `git branch -d` drops it with the branch.
pub fn base_key(branch: &str) -> String {
    format!("branch.{branch}.gf-base")
}

/// The branch the oldest reflog entry of a branch says it was made from
/// (`branch: Created from develop`); `None` for HEAD or a commit id, which
/// do not tell.
pub fn parse_created_from(subject: &str) -> Option<&str> {
    let from = subject.trim().strip_prefix("branch: Created from ")?.trim();
    let is_id = from.len() >= 7 && from.chars().all(|c| c.is_ascii_hexdigit());
    (!from.is_empty() && from != "HEAD" && !from.starts_with("refs/tags/") && !is_id)
        .then_some(from)
}

/// Changed files whose modification time is read for
/// [`WorktreeStatus::last_activity`]; more are not worth the stat calls.
pub(crate) const ACTIVITY_SAMPLE: usize = 256;

impl CliBackend {
    /// Reads a command's stdout in `dir`, up to the byte limit used for
    /// diffs. A failed or truncated read yields no paths.
    async fn read_in(&self, dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
        let cmd = self
            .command(args.iter().copied())
            .current_dir(dir)
            .read_only()
            .allow_failure();
        let opts = RunOptions {
            max_stdout: Some(DIFF_LIMIT),
            ..RunOptions::default()
        };
        let out = self.runner().run_with(&cmd, opts).await.ok()?;
        out.success().then_some(out.stdout)
    }

    /// A command's first output line in `dir`, when git succeeds and it
    /// is not empty.
    async fn line_in(&self, dir: &Path, args: &[&str]) -> Option<String> {
        let out = self.read_in(dir, args).await?;
        let line = String::from_utf8_lossy(&out)
            .lines()
            .next()?
            .trim()
            .to_owned();
        (!line.is_empty()).then_some(line)
    }

    /// The local branch for `name` (`develop`, or `origin/develop` when a
    /// local `develop` exists); the remote branch itself when there is no
    /// local one; `None` when it is neither.
    async fn branch_named(&self, dir: &Path, name: &str) -> Option<String> {
        let exists = |full: String| async move {
            self.read_in(dir, &["rev-parse", "--verify", "-q", &full])
                .await
                .is_some()
        };
        let name = name.strip_prefix("refs/heads/").unwrap_or(name);
        if exists(format!("refs/heads/{name}")).await {
            return Some(name.to_owned());
        }
        let remote = name.strip_prefix("refs/remotes/").unwrap_or(name);
        if !exists(format!("refs/remotes/{remote}")).await {
            return None;
        }
        if let Some((_, local)) = remote.split_once('/')
            && exists(format!("refs/heads/{local}")).await
        {
            return Some(local.to_owned());
        }
        Some(remote.to_owned())
    }

    /// The branch `branch` was made from, and whether that is a guess:
    ///
    /// 1. the one recorded for it: `branch.<b>.gf-base` (set by gf when it
    ///    makes the branch, or by the user), else gh's `gh-merge-base`;
    /// 2. the one its creation names (`created`, the oldest reflog entry);
    /// 3. a guess: of the default and the usual branches, the one HEAD has
    ///    the fewest own commits against (then the fewest missing ones) —
    ///    where it most likely left. Last, the default itself.
    pub(crate) async fn base_of(
        &self,
        dir: &Path,
        branch: &str,
        created: Option<&str>,
        default: Option<&str>,
    ) -> (Option<String>, bool) {
        let (gf_key, gh_key) = (base_key(branch), format!("branch.{branch}.gh-merge-base"));
        let gf_args = ["config", "--get", gf_key.as_str()];
        let gh_args = ["config", "--get", gh_key.as_str()];
        let (gf, gh) = tokio::join!(self.line_in(dir, &gf_args), self.line_in(dir, &gh_args));
        if let Some(base) = gf.or(gh).filter(|b| b != branch) {
            return (Some(base), false);
        }
        if let Some(from) = created.and_then(parse_created_from)
            && let Some(base) = self.branch_named(dir, from).await
            && base != branch
        {
            return (Some(base), false);
        }
        let mut best: Option<((u32, u32), String)> = None;
        let mut tried: Vec<&str> = Vec::new();
        for name in default.into_iter().chain(USUAL_BASES) {
            if name == branch || tried.contains(&name) {
                continue;
            }
            tried.push(name);
            // The default may be a remote branch; the others are local.
            let rev = if Some(name) == default {
                name.to_owned()
            } else {
                format!("refs/heads/{name}")
            };
            let range = format!("{rev}...HEAD");
            let args = [
                "rev-list",
                "--left-right",
                "--count",
                "--end-of-options",
                &range,
            ];
            if let Some((behind, ahead)) = self
                .read_in(dir, &args)
                .await
                .as_deref()
                .and_then(parse_left_right)
                && best.as_ref().is_none_or(|(key, _)| (ahead, behind) < *key)
            {
                best = Some(((ahead, behind), name.to_owned()));
            }
        }
        (
            best.map(|(_, name)| name).or(default.map(str::to_owned)),
            true,
        )
    }
}

/// The file at a repository's root that lists, in `.gitignore` syntax, the
/// git-ignored files each new worktree gets a copy of (`.env` …). The same
/// file Claude Code, Conductor and `git-worktreeinclude` read.
pub const INCLUDE_FILE: &str = ".worktreeinclude";

/// What [`WorktreeOps::copy_included`] did. Paths are relative to the trees.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IncludedCopy {
    /// The source tree has an [`INCLUDE_FILE`]; without one nothing is read.
    pub listed: bool,
    pub copied: Vec<PathBuf>,
    /// Already in the new tree: left as they are.
    pub kept: Vec<PathBuf>,
    /// With the reason.
    pub failed: Vec<(PathBuf, String)>,
}

impl IncludedCopy {
    /// The files that are there, one line each, for the command log.
    pub fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .copied
            .iter()
            .map(|p| format!("copied {}", p.display()))
            .chain(
                self.kept
                    .iter()
                    .map(|p| format!("kept {}: already in the worktree", p.display())),
            )
            .collect();
        if lines.is_empty() && self.failed.is_empty() && self.listed {
            lines.push(format!("{INCLUDE_FILE} matched no ignored file"));
        }
        lines
    }

    /// The files that could not be copied, one line each with the reason.
    pub fn failure_lines(&self) -> Vec<String> {
        self.failed
            .iter()
            .map(|(p, e)| format!("could not copy {}: {e}", p.display()))
            .collect()
    }
}

/// Copies the files at `paths` (relative) from the tree at `source` into the
/// one at `dest`, making their folders. Only regular files, and never over
/// something already in `dest` or through a link on the way there.
pub fn copy_included(source: &Path, dest: &Path, paths: &[PathBuf]) -> IncludedCopy {
    let mut copy = IncludedCopy::default();
    for rel in paths {
        let from = source.join(rel);
        if !std::fs::symlink_metadata(&from).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let to = dest.join(rel);
        if std::fs::symlink_metadata(&to).is_ok() {
            copy.kept.push(rel.clone());
            continue;
        }
        // A linked folder in the new tree would put the file somewhere else.
        let linked = rel
            .ancestors()
            .skip(1)
            .filter(|a| !a.as_os_str().is_empty())
            .any(|a| std::fs::symlink_metadata(dest.join(a)).is_ok_and(|m| m.is_symlink()));
        if linked {
            copy.failed
                .push((rel.clone(), "a folder on the way is a link".into()));
            continue;
        }
        let made = to
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::copy(&from, &to).map(drop));
        match made {
            Ok(()) => copy.copied.push(rel.clone()),
            Err(e) => copy.failed.push((rel.clone(), e.to_string())),
        }
    }
    copy
}

impl WorktreeOps for CliBackend {
    async fn worktrees(&self) -> Result<Vec<Worktree>> {
        parse_worktrees(
            &self
                .read(self.command(["worktree", "list", "--porcelain"]))
                .await?,
        )
    }

    async fn add_worktree(
        &self,
        path: &Path,
        branch: &str,
        new_branch: bool,
        start: Option<&str>,
    ) -> Result<()> {
        let mut cmd = self.command(["worktree", "add"]);
        if new_branch {
            cmd = cmd.args(["-b", branch]).arg(path);
            if let Some(start) = start {
                cmd = cmd.arg(start);
            }
        } else {
            cmd = cmd.arg(path).arg(branch);
        }
        // Where a new branch starts is where its work goes back to: keep it,
        // since git's own record says only "Created from HEAD".
        let from = match (new_branch, start) {
            (false, _) => None,
            (true, Some(start)) => Some(start.to_owned()),
            (true, None) => {
                self.line_in(self.cwd(), &["symbolic-ref", "-q", "--short", "HEAD"])
                    .await
            }
        };
        let from = match from {
            Some(from) => self.branch_named(self.cwd(), &from).await,
            None => None,
        };
        self.exec(cmd).await?;
        if let Some(from) = from.filter(|f| f != branch) {
            self.set_branch_base(branch, Some(&from)).await?;
        }
        Ok(())
    }

    async fn set_branch_base(&self, branch: &str, base: Option<&str>) -> Result<()> {
        let key = base_key(branch);
        match base {
            Some(base) => {
                self.exec(self.command(["config", &key, base])).await?;
            }
            // Nothing to unset is fine: back to guessing either way.
            None => {
                self.exec(self.command(["config", "--unset", &key]).allow_failure())
                    .await?;
            }
        }
        Ok(())
    }

    async fn worktree_status(&self, path: &Path, base: Option<&str>) -> Result<WorktreeStatus> {
        let status_args = [
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ];
        let log_args = ["log", "-1", "--format=%H%x00%ct%x00%s"];
        let (status, last) = tokio::join!(
            self.read_in(path, &status_args),
            self.read_in(path, &log_args)
        );
        let status = status.ok_or_else(|| {
            GfError::parse("worktree status", format!("cannot read {}", path.display()))
        })?;
        let report = parse_status(&status)?;
        let mut s = status_counts(&report);
        s.last_commit = last.as_deref().and_then(parse_last_commit);
        // The branch's reflog, newest first: its creation is the last line.
        let reflog: Vec<(String, String)> = match report.head.branch.as_deref() {
            Some(branch) => {
                let full = format!("refs/heads/{branch}");
                let args = [
                    "reflog",
                    "show",
                    "--format=%H%x00%gs",
                    "--end-of-options",
                    &full,
                ];
                self.read_in(path, &args)
                    .await
                    .map_or_else(Vec::new, |o| parse_reflog(&o))
            }
            None => Vec::new(),
        };
        // The default base itself goes nowhere; nor does a tree without
        // commits.
        let on_default = base.is_some_and(|b| {
            report.head.branch.as_deref() == Some(b) || report.head.upstream.as_deref() == Some(b)
        });
        let (tree_base, guessed) = match report.head.branch.as_deref() {
            _ if on_default || s.last_commit.is_none() => (None, false),
            Some(branch) => {
                let created = reflog.last().map(|(_, m)| m.as_str());
                self.base_of(path, branch, created, base).await
            }
            None => (base.map(str::to_owned), true),
        };
        if let Some(base) = tree_base {
            let range = format!("{base}...HEAD");
            let count = [
                "rev-list",
                "--left-right",
                "--count",
                "--end-of-options",
                &range,
            ];
            let fork = ["merge-base", "--end-of-options", &base, "HEAD"];
            let (count, fork) = tokio::join!(self.read_in(path, &count), self.read_in(path, &fork));
            if let Some((behind, ahead)) = count.as_deref().and_then(parse_left_right) {
                s.behind_base = behind;
                s.ahead_of_base = ahead;
                if ahead > 0 {
                    // Without the merges that brought the base in.
                    let range = format!("{base}..HEAD");
                    let args = [
                        "rev-list",
                        "--count",
                        "--no-merges",
                        "--end-of-options",
                        &range,
                    ];
                    if let Some(n) = self.line_in(path, &args).await.and_then(|l| l.parse().ok()) {
                        s.ahead_of_base = n;
                    }
                }
                s.merge_base = fork.and_then(|o| String::from_utf8_lossy(&o).trim().parse().ok());
                if s.ahead_of_base == 0 {
                    for id in made_here(&reflog).take(MADE_HERE_CHECKED) {
                        let args = ["merge-base", "--is-ancestor", id, &base];
                        if self.read_in(path, &args).await.is_some() {
                            s.work_in_base = true;
                            break;
                        }
                    }
                }
                s.base = Some(base);
                s.base_guessed = guessed;
            }
        }
        // What its commits changed, when it has any of its own.
        let committed = match (&s.merge_base, s.ahead_of_base) {
            (Some(fork), 1..) => {
                let fork = fork.to_string();
                let args = [
                    "diff",
                    "--name-only",
                    "-z",
                    "--no-renames",
                    "--no-ext-diff",
                    "--end-of-options",
                    &fork,
                    "HEAD",
                ];
                self.read_in(path, &args)
                    .await
                    .map(|o| crate::ops::log::parse_tree_paths(&o))
            }
            _ => Some(Vec::new()),
        };
        // Its commits are not in the base, which moved on: did their
        // changes get there another way (squash, rebase, cherry-pick)?
        if let (Some(base), Some(paths)) = (&s.base, &committed)
            && s.ahead_of_base > 0
            && s.behind_base > 0
            && !paths.is_empty()
        {
            let args = [
                "diff",
                "--name-only",
                "-z",
                "--no-renames",
                "--no-ext-diff",
                "--end-of-options",
                base,
                "HEAD",
            ];
            if let Some(out) = self.read_in(path, &args).await {
                let differ = crate::ops::log::parse_tree_paths(&out);
                s.squashed = squashed(paths, &differ);
            }
        }
        s.touched = committed.map(|mut paths| {
            if s.squashed {
                paths.clear();
            }
            touched(&report, paths)
        });
        let mut newest = None;
        for e in report
            .entries
            .iter()
            .filter(|e| !matches!(e, StatusEntry::Ignored { .. }))
            .take(ACTIVITY_SAMPLE)
        {
            if let Ok(t) = tokio::fs::symlink_metadata(path.join(e.path()))
                .await
                .and_then(|m| m.modified())
            {
                newest = newest.max(Some(t));
            }
        }
        s.last_activity = newest;
        Ok(s)
    }

    async fn pull_rebases(&self, path: &Path, branch: &str) -> Result<bool> {
        let key = format!("branch.{branch}.rebase");
        let value = match self.line_in(path, &["config", "--get", &key]).await {
            Some(v) => Some(v),
            None => {
                self.line_in(path, &["config", "--get", "pull.rebase"])
                    .await
            }
        };
        Ok(value.as_deref().is_some_and(rebases))
    }

    async fn default_base(&self) -> Result<Option<String>> {
        let cwd = self.cwd().to_path_buf();
        let exists = |name: String| {
            let cwd = cwd.clone();
            async move {
                let full = format!("refs/heads/{name}");
                self.read_in(&cwd, &["rev-parse", "--verify", "-q", &full])
                    .await
                    .map(|_| name)
            }
        };
        if let Some(remote_head) = self
            .read_in(
                &cwd,
                &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
            )
            .await
        {
            let remote_head = String::from_utf8_lossy(&remote_head).trim().to_owned();
            if let Some((_, short)) = remote_head.split_once('/')
                && let Some(local) = exists(short.to_owned()).await
            {
                return Ok(Some(local));
            }
            if !remote_head.is_empty() {
                return Ok(Some(remote_head));
            }
        }
        for name in ["main", "master"] {
            if let Some(local) = exists(name.to_owned()).await {
                return Ok(Some(local));
            }
        }
        Ok(None)
    }

    async fn lock_worktree(&self, path: &Path, locked: bool) -> Result<()> {
        let verb = if locked { "lock" } else { "unlock" };
        self.exec(self.command(["worktree", verb]).arg(path))
            .await?;
        Ok(())
    }

    async fn remove_worktree(&self, path: &Path, force: bool) -> Result<()> {
        let mut cmd = self.command(["worktree", "remove"]);
        if force {
            cmd = cmd.arg("--force");
        }
        self.exec(cmd.arg(path)).await?;
        Ok(())
    }

    async fn prune_worktrees(&self) -> Result<()> {
        self.exec(self.command(["worktree", "prune"])).await?;
        Ok(())
    }

    async fn copy_included(&self, dest: &Path) -> Result<IncludedCopy> {
        let source = self.cwd().to_path_buf();
        let include = source.join(INCLUDE_FILE);
        if !tokio::fs::metadata(&include)
            .await
            .is_ok_and(|m| m.is_file())
        {
            return Ok(IncludedCopy::default());
        }
        let started = std::time::Instant::now();
        // git's own matcher reads the patterns: the untracked files they
        // select…
        let mut exclude_from = std::ffi::OsString::from("--exclude-from=");
        exclude_from.push(&include);
        let selected = self
            .read(
                self.command(["ls-files", "-z", "--others", "--ignored"])
                    .arg(exclude_from),
            )
            .await?;
        // …that git ignores too, so a pattern never copies a file meant to
        // be committed.
        let paths = if selected.is_empty() {
            Vec::new()
        } else {
            let cmd = self
                .command(["check-ignore", "-z", "--stdin"])
                .stdin(selected)
                .read_only()
                .allow_failure();
            let out = self.runner().run(&cmd).await?;
            match out.exit_code {
                Some(0) => crate::ops::log::parse_tree_paths(&out.stdout),
                // None of them is ignored.
                Some(1) => Vec::new(),
                _ => {
                    return Err(GfError::Other(format!(
                        "git check-ignore: {}",
                        out.stderr.trim()
                    )));
                }
            }
        };
        let to = dest.to_path_buf();
        let mut copy = tokio::task::spawn_blocking(move || copy_included(&source, &to, &paths))
            .await
            .map_err(|e| GfError::Other(format!("copying {INCLUDE_FILE} files: {e}")))?;
        copy.listed = true;
        // Two entries, so the files that are there do not read as errors
        // next to the ones that failed.
        let argv = || vec!["gf".into(), INCLUDE_FILE.into(), dest.display().to_string()];
        let (lines, failures) = (copy.lines(), copy.failure_lines());
        if !lines.is_empty() {
            self.runner().report_in_process(
                Some(self.handle().id),
                argv(),
                &lines,
                0,
                started.elapsed(),
            );
        }
        if !failures.is_empty() {
            self.runner().report_in_process(
                Some(self.handle().id),
                argv(),
                &failures,
                1,
                started.elapsed(),
            );
        }
        Ok(copy)
    }
}

/// A `pull.rebase` / `branch.<b>.rebase` value asks for a rebase: any but
/// git's false (`false`, `no`, `off`, `0`).
pub(crate) fn rebases(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "false" | "no" | "off" | "0" | ""
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn included_files_are_copied_once_and_never_over_others() {
        use super::copy_included;
        use std::path::PathBuf;
        let source = tempfile::tempdir().expect("source");
        let dest = tempfile::tempdir().expect("dest");
        let write = |root: &std::path::Path, rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
            std::fs::write(path, text).expect("write");
        };
        write(source.path(), ".env", "A=1\n");
        write(source.path(), "config/local/db.toml", "url = 1\n");
        write(source.path(), "taken", "mine\n");
        write(dest.path(), "taken", "theirs\n");
        std::fs::create_dir_all(source.path().join("a-folder")).expect("folder");
        let paths: Vec<PathBuf> = [".env", "config/local/db.toml", "taken", "gone", "a-folder"]
            .into_iter()
            .map(PathBuf::from)
            .collect();

        let copy = copy_included(source.path(), dest.path(), &paths);
        assert_eq!(
            copy.copied,
            vec![PathBuf::from(".env"), PathBuf::from("config/local/db.toml")]
        );
        assert_eq!(copy.kept, vec![PathBuf::from("taken")]);
        assert!(copy.failed.is_empty());
        let read = |rel: &str| std::fs::read_to_string(dest.path().join(rel)).expect("read");
        assert_eq!(read("config/local/db.toml"), "url = 1\n");
        assert_eq!(read("taken"), "theirs\n", "never copied over");

        // Again: everything is there already.
        let again = copy_included(source.path(), dest.path(), &paths);
        assert!(again.copied.is_empty());
        assert_eq!(again.kept.len(), 3);
    }

    #[test]
    fn included_copy_lines_say_what_happened() {
        use super::IncludedCopy;
        let copy = IncludedCopy {
            listed: true,
            copied: vec![".env".into()],
            kept: vec!["b".into()],
            failed: vec![("c".into(), "denied".into())],
        };
        assert_eq!(
            copy.lines(),
            vec!["copied .env", "kept b: already in the worktree"]
        );
        assert_eq!(copy.failure_lines(), vec!["could not copy c: denied"]);
        let only_failed = IncludedCopy {
            listed: true,
            failed: vec![("c".into(), "denied".into())],
            ..IncludedCopy::default()
        };
        assert!(only_failed.lines().is_empty(), "not 'matched nothing'");
        let none = IncludedCopy {
            listed: true,
            ..IncludedCopy::default()
        };
        assert_eq!(
            none.lines(),
            vec![".worktreeinclude matched no ignored file"]
        );
        assert!(IncludedCopy::default().lines().is_empty());
    }

    #[test]
    fn touched_joins_commits_and_changes_without_a_file_count_limit() {
        use gf_core::model::{StatusEntry, StatusReport};
        use std::path::PathBuf;
        let report = StatusReport {
            entries: vec![
                StatusEntry::Untracked {
                    path: "new/file".into(),
                },
                StatusEntry::Ignored {
                    path: "target/".into(),
                },
            ],
            ..StatusReport::default()
        };
        let committed = vec![PathBuf::from("b.rs"), PathBuf::from("a.rs")];
        assert_eq!(
            super::touched(&report, committed),
            vec![
                PathBuf::from("a.rs"),
                PathBuf::from("b.rs"),
                PathBuf::from("new/file")
            ]
        );
        let many = (0..2001).map(|i| PathBuf::from(format!("f{i}"))).collect();
        assert_eq!(super::touched(&report, many).len(), 2002);
    }

    #[test]
    fn pull_rebase_values_read_like_git() {
        for v in [
            "true",
            "merges",
            "interactive",
            "i",
            "m",
            "yes",
            "1",
            "TRUE",
        ] {
            assert!(super::rebases(v), "{v}");
        }
        for v in ["false", "no", "off", "0", " False "] {
            assert!(!super::rebases(v), "{v}");
        }
    }

    use super::*;

    #[test]
    fn parses_porcelain_records() {
        let id = "a".repeat(40);
        let out = format!(
            "worktree /repo\nHEAD {id}\nbranch refs/heads/main\n\n\
             worktree /repo-wt\nHEAD {id}\ndetached\nlocked reason\n\n\
             worktree /gone\nHEAD {id}\nbranch refs/heads/old\nprunable gitdir file points to non-existent location\n"
        );
        let w = parse_worktrees(out.as_bytes()).expect("parse");
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].branch.as_deref(), Some("main"));
        assert!(w[1].branch.is_none() && w[1].locked);
        assert!(w[2].prunable);
    }

    #[test]
    fn reads_where_a_branch_was_made_from() {
        assert_eq!(
            parse_created_from("branch: Created from develop"),
            Some("develop")
        );
        assert_eq!(
            parse_created_from("branch: Created from origin/main\n"),
            Some("origin/main")
        );
        assert_eq!(parse_created_from("branch: Created from HEAD"), None);
        assert_eq!(parse_created_from("branch: Created from 2bd778b"), None);
        assert_eq!(parse_created_from("commit: fix"), None);
        assert_eq!(base_key("feature/x"), "branch.feature/x.gf-base");
    }

    #[test]
    fn parses_the_last_commit_and_left_right_counts() {
        let id = "b".repeat(40);
        let c = parse_last_commit(format!("{id}\x001700000000\x00fix: a\x00b\n").as_bytes())
            .expect("commit");
        assert_eq!(c.time, 1_700_000_000);
        assert_eq!(c.subject, "fix: a\0b", "the subject keeps what follows");
        assert!(parse_last_commit(b"").is_none(), "unborn branch");
        assert_eq!(parse_left_right(b"3\t5\n"), Some((3, 5)));
        assert_eq!(parse_left_right(b""), None);
    }

    #[test]
    fn counts_status_entries_and_merged() {
        use gf_core::model::{ChangeKind, ConflictKind, HeadInfo};
        let changed = |index, worktree| StatusEntry::Changed {
            path: "f".into(),
            orig_path: None,
            index,
            worktree,
        };
        let report = StatusReport {
            head: HeadInfo {
                ahead: 2,
                ..HeadInfo::default()
            },
            entries: vec![
                changed(ChangeKind::Modified, ChangeKind::Modified),
                changed(ChangeKind::Unmodified, ChangeKind::Modified),
                StatusEntry::Untracked { path: "u".into() },
                StatusEntry::Ignored { path: "i".into() },
                StatusEntry::Conflicted {
                    path: "c".into(),
                    kind: ConflictKind::BothModified,
                },
            ],
        };
        let mut s = status_counts(&report);
        assert_eq!(
            (s.staged, s.unstaged, s.untracked, s.conflicts),
            (1, 2, 1, 1)
        );
        assert_eq!(s.changed(), 5);
        assert_eq!(s.ahead, 2);
        assert!(!s.merged(), "no base");
        s.base = Some("main".into());
        s.behind_base = 1;
        assert!(!s.merged(), "a fresh branch the base moved past");
        s.work_in_base = true;
        assert!(s.merged());
        s.behind_base = 0;
        assert!(s.merged(), "fast-forwarded into the base");
        s.ahead_of_base = 1;
        assert!(!s.merged(), "work not in main yet");
        assert!(!s.all_in_base());
        s.squashed = true;
        assert!(s.merged() && s.all_in_base(), "squashed into main");
    }

    #[test]
    fn made_here_keeps_commits_and_rebases_that_carried_work() {
        let log = super::parse_reflog(
            b"a1\0merge main: Fast-forward\n\
              a2\0rebase (finish): refs/heads/t onto a2\n\
              a3\0rebase -i (finish): refs/heads/t onto b9\n\
              a4\0reset: moving to main\n\
              a5\0commit (amend): w\n\
              a6\0cherry-pick: x\n\
              a7\0revert: y\n\
              a8\0merge main: Merge made by the 'ort' strategy.\n\
              a9\0gf: undo\n\
              b0\0\n\
              b1\0commit: w\n\
              b2\0branch: Created from main\n",
        );
        assert_eq!(log.len(), 11, "the entry without a message is dropped");
        assert_eq!(
            log.last().map(|(_, m)| m.as_str()),
            Some("branch: Created from main")
        );
        let own: Vec<&str> = super::made_here(&log).collect();
        assert_eq!(own, ["a3", "a5", "a6", "a7", "b1"]);
    }

    #[test]
    fn squashed_when_no_committed_file_differs_from_the_base() {
        use std::path::PathBuf;
        let p = |v: &[&str]| v.iter().map(PathBuf::from).collect::<Vec<_>>();
        assert!(super::squashed(&p(&["b.rs", "a.rs"]), &p(&["c.rs"])));
        assert!(!super::squashed(&p(&["b.rs", "a.rs"]), &p(&["a.rs"])));
        assert!(
            !super::squashed(&[], &[]),
            "nothing committed, nothing squashed"
        );
    }
}
