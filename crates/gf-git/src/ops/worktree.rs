//! Linked working trees (`git worktree`): the list, what each one is doing
//! right now ([`WorktreeStatus`]) and add / remove / lock / prune.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gf_core::model::{Oid, StatusEntry, StatusReport};
use gf_core::{GfError, Result};

use crate::backend::{CliBackend, WorktreeOps};
use crate::cli::parse::status::parse_status;

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
    /// Untracked files and folders (folders count once).
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
    pub ahead_of_base: u32,
    /// Commits on the base that HEAD does not have.
    pub behind_base: u32,
    /// Where HEAD left the base: comparing from here shows only the work
    /// done in this tree.
    pub merge_base: Option<Oid>,
    /// The branch moved after it was created (its reflog has more than the
    /// creation): it had work of its own, so being in the base means it
    /// was merged, not that it is new.
    pub had_work: bool,
    pub last_commit: Option<LastCommit>,
    /// Newest modification time among the changed files: whether someone
    /// (an agent) is editing in this tree right now.
    pub last_activity: Option<SystemTime>,
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
    /// or fast-forwarded into it): the tree can go.
    pub fn merged(&self) -> bool {
        self.base.is_some() && self.ahead_of_base == 0 && self.had_work
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

/// Parses `git rev-list --left-right --count A...B`: (only in A, only in B).
pub fn parse_left_right(out: &[u8]) -> Option<(u32, u32)> {
    let text = String::from_utf8_lossy(out);
    let mut n = text.split_whitespace().map(str::parse);
    Some((n.next()?.ok()?, n.next()?.ok()?))
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
    /// Reads a command's stdout in `dir`; `None` when git fails.
    async fn read_in(&self, dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
        let cmd = self
            .command(args.iter().copied())
            .current_dir(dir)
            .read_only()
            .allow_failure();
        let out = self.runner().run(&cmd).await.ok()?;
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
            "--untracked-files=normal",
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
        let reflog: Vec<String> = match report.head.branch.as_deref() {
            Some(branch) => {
                let full = format!("refs/heads/{branch}");
                let args = ["reflog", "show", "--format=%gs", "--end-of-options", &full];
                self.read_in(path, &args).await.map_or_else(Vec::new, |o| {
                    String::from_utf8_lossy(&o)
                        .lines()
                        .filter(|l| !l.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
            }
            None => Vec::new(),
        };
        // It moved after it was made: it had work of its own, so being in
        // the base means it was merged, not that it is new.
        s.had_work = reflog.len() > 1;
        // The default base itself goes nowhere; nor does a tree without
        // commits.
        let on_default = base.is_some_and(|b| {
            report.head.branch.as_deref() == Some(b) || report.head.upstream.as_deref() == Some(b)
        });
        let (tree_base, guessed) = match report.head.branch.as_deref() {
            _ if on_default || s.last_commit.is_none() => (None, false),
            Some(branch) => {
                self.base_of(path, branch, reflog.last().map(String::as_str), base)
                    .await
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
                s.merge_base = fork.and_then(|o| String::from_utf8_lossy(&o).trim().parse().ok());
                s.base = Some(base);
                s.base_guessed = guessed;
            }
        }
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
}

#[cfg(test)]
mod tests {
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
        s.had_work = true;
        assert!(s.merged());
        s.behind_base = 0;
        assert!(s.merged(), "fast-forwarded into the base");
        s.ahead_of_base = 1;
        assert!(!s.merged(), "work not in main yet");
    }
}
