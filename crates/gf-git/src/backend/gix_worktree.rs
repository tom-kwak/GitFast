//! What each working tree is doing ([`WorktreeStatus`]) and the default
//! base, through gix: the worktrees view reads every tree at once, and the
//! CLI needs six to a dozen git processes per tree for it. Must equal the
//! CLI's `WorktreeOps` (see `tests/gix_contract.rs`).

use std::path::Path;
use std::time::SystemTime;

use gf_core::Result;
use gf_core::model::StatusEntry;
use gix::ObjectId;
use gix::bstr::ByteSlice;
use gix::refs::TargetRef;

use super::gix_backend::{count, err, head_id, oid, resolve, text, unsupported};
use super::gix_status::status_with;
use crate::cli::parse::log::split_message;
use crate::ops::worktree::{
    ACTIVITY_SAMPLE, LastCommit, MADE_HERE_CHECKED, TOUCHED_CAP, USUAL_BASES, WorktreeStatus,
    made_here, parse_created_from, status_counts, touched,
};

/// `git config --get <section>.<sub>.<key>`: the last value, trimmed;
/// `None` when unset or empty.
fn config_value(repo: &gix::Repository, sub: &str, key: &str) -> Option<String> {
    let config = repo.config_snapshot();
    let value = config
        .plumbing()
        .string_by("branch", Some(sub.as_bytes().as_bstr()), key)?;
    let value = text(value.as_ref()).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

/// `git rev-parse --verify -q <rev>` succeeds.
fn exists(repo: &gix::Repository, rev: &str) -> bool {
    repo.rev_parse_single(rev).is_ok()
}

/// See `CliBackend::branch_named`.
fn branch_named(repo: &gix::Repository, name: &str) -> Option<String> {
    let name = name.strip_prefix("refs/heads/").unwrap_or(name);
    if exists(repo, &format!("refs/heads/{name}")) {
        return Some(name.to_owned());
    }
    let remote = name.strip_prefix("refs/remotes/").unwrap_or(name);
    if !exists(repo, &format!("refs/remotes/{remote}")) {
        return None;
    }
    if let Some((_, local)) = remote.split_once('/')
        && exists(repo, &format!("refs/heads/{local}"))
    {
        return Some(local.to_owned());
    }
    Some(remote.to_owned())
}

/// `git rev-list --left-right --count <rev>...HEAD`: (behind, ahead);
/// `None` when `rev` does not resolve.
fn left_right(repo: &gix::Repository, rev: &str, head: ObjectId) -> Option<(u32, u32)> {
    let other = resolve(repo, rev).ok()?;
    Some((
        count(repo, other, head).ok()?,
        count(repo, head, other).ok()?,
    ))
}

/// See `CliBackend::base_of`; the same steps, in process.
fn base_of(
    repo: &gix::Repository,
    head: ObjectId,
    branch: &str,
    created: Option<&str>,
    default: Option<&str>,
) -> (Option<String>, bool) {
    // `branch.<b>.gf-base` ([`base_key`](crate::ops::worktree::base_key)), else gh's `gh-merge-base`.
    let gf = config_value(repo, branch, "gf-base")
        .or_else(|| config_value(repo, branch, "gh-merge-base"));
    if let Some(base) = gf.filter(|b| b != branch) {
        return (Some(base), false);
    }
    if let Some(from) = created.and_then(parse_created_from)
        && let Some(base) = branch_named(repo, from)
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
        let rev = if Some(name) == default {
            name.to_owned()
        } else {
            format!("refs/heads/{name}")
        };
        if let Some((behind, ahead)) = left_right(repo, &rev, head)
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

/// `git reflog show --format=%H%x00%gs refs/heads/<branch>` as
/// [`parse_reflog`](crate::ops::worktree::parse_reflog) reads it: `(new
/// id, message)`, newest first, without empty messages.
fn branch_reflog(repo: &gix::Repository, branch: &str) -> Result<Vec<(String, String)>> {
    let Some(reference) = repo
        .try_find_reference(format!("refs/heads/{branch}").as_str())
        .map_err(err)?
    else {
        return Ok(Vec::new());
    };
    let mut platform = reference.log_iter();
    let Some(lines) = platform.rev().map_err(err)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for line in lines {
        let line = line.map_err(err)?;
        let message = text(line.message.as_ref());
        if !message.is_empty() {
            out.push((line.new_oid.to_string(), message));
        }
    }
    Ok(out)
}

fn last_commit(repo: &gix::Repository, head: ObjectId) -> Result<LastCommit> {
    let c = repo.find_commit(head).map_err(err)?;
    let (subject, _) = split_message(&String::from_utf8_lossy(c.message_raw().map_err(err)?));
    Ok(LastCommit {
        id: oid(head),
        subject,
        time: c.committer().map_err(err)?.time().map_err(err)?.seconds,
    })
}

/// [`WorktreeStatus`] of the tree at `path`, like the CLI's.
pub(super) fn worktree_status(path: &Path, base: Option<&str>) -> Result<WorktreeStatus> {
    let repo = gix::open(path).map_err(err)?;
    // gix reads a differently named key for this; leave it to git.
    if repo
        .config_snapshot()
        .string("diff.ignoreSubmodules")
        .is_some()
    {
        return unsupported("diff.ignoreSubmodules");
    }
    let report = status_with(
        &repo,
        gix::status::UntrackedFiles::Collapsed,
        gix::status::Submodule::AsConfigured { check_dirty: true },
    )?;
    let mut s = status_counts(&report);
    let head = head_id(&repo)?;
    if let Some(head) = head {
        s.last_commit = Some(last_commit(&repo, head)?);
    }
    let reflog = match report.head.branch.as_deref() {
        Some(branch) => branch_reflog(&repo, branch)?,
        None => Vec::new(),
    };
    let on_default = base.is_some_and(|b| {
        report.head.branch.as_deref() == Some(b) || report.head.upstream.as_deref() == Some(b)
    });
    let (tree_base, guessed) = match (report.head.branch.as_deref(), head) {
        (_, None) => (None, false),
        _ if on_default => (None, false),
        (Some(branch), Some(head)) => base_of(
            &repo,
            head,
            branch,
            reflog.last().map(|(_, m)| m.as_str()),
            base,
        ),
        (None, Some(_)) => (base.map(str::to_owned), true),
    };
    let mut base_id = None;
    if let (Some(base), Some(head)) = (tree_base, head)
        && let Some((behind, ahead)) = left_right(&repo, &base, head)
    {
        let other = resolve(&repo, &base)?;
        s.behind_base = behind;
        s.ahead_of_base = if ahead > 0 {
            // Without the merges that brought the base in.
            count_no_merges(&repo, head, other)?
        } else {
            0
        };
        s.merge_base = repo
            .merge_base(other, head)
            .ok()
            .flatten()
            .map(|id| oid(id.detach()));
        if s.ahead_of_base == 0 {
            s.work_in_base = made_here(&reflog)
                .take(MADE_HERE_CHECKED)
                .filter_map(|id| ObjectId::from_hex(id.as_bytes()).ok())
                .any(|id| is_ancestor(&repo, id, other));
        }
        s.base = Some(base);
        s.base_guessed = guessed;
        base_id = Some(other);
    }
    let committed = match (&s.merge_base, head, s.ahead_of_base) {
        (Some(fork), Some(head), 1..) => committed_paths(&repo, fork, head)?,
        _ => Vec::new(),
    };
    // See the CLI's: changes of its commits that got into the base
    // another way.
    if let (Some(other), Some(head)) = (base_id, head)
        && s.behind_base > 0
        && !committed.is_empty()
        && committed.len() <= TOUCHED_CAP
    {
        s.squashed = same_in_both(&repo, &committed, other, head)?;
    }
    s.touched = touched(&report, committed);
    let mut newest: Option<SystemTime> = None;
    for e in report
        .entries
        .iter()
        .filter(|e| !matches!(e, StatusEntry::Ignored { .. }))
        .take(ACTIVITY_SAMPLE)
    {
        if let Ok(t) = std::fs::symlink_metadata(path.join(e.path())).and_then(|m| m.modified()) {
            newest = newest.max(Some(t));
        }
    }
    s.last_activity = newest;
    Ok(s)
}

/// `git merge-base --is-ancestor <id> <of>`.
fn is_ancestor(repo: &gix::Repository, id: ObjectId, of: ObjectId) -> bool {
    id == of
        || repo
            .merge_base(id, of)
            .is_ok_and(|base| base.is_some_and(|b| b.detach() == id))
}

/// `git rev-list --count --no-merges <tip> --not <hidden>`.
fn count_no_merges(repo: &gix::Repository, tip: ObjectId, hidden: ObjectId) -> Result<u32> {
    let mut n = 0u32;
    for info in repo
        .rev_walk([tip])
        .with_hidden([hidden])
        .all()
        .map_err(err)?
    {
        if info.map_err(err)?.parent_ids.len() < 2 {
            n += 1;
        }
    }
    Ok(n)
}

/// Every one of `paths` is the same in `a` and `b` (or missing from
/// both): `git diff --name-only --no-renames a b` names none of them.
fn same_in_both(
    repo: &gix::Repository,
    paths: &[std::path::PathBuf],
    a: ObjectId,
    b: ObjectId,
) -> Result<bool> {
    let tree = |id: ObjectId| -> Result<gix::Tree<'_>> {
        repo.find_commit(id).map_err(err)?.tree().map_err(err)
    };
    let (a, b) = (tree(a)?, tree(b)?);
    for path in paths {
        let entry =
            |t: &gix::Tree<'_>| -> Result<Option<(ObjectId, gix::object::tree::EntryMode)>> {
                Ok(t.lookup_entry_by_path(path)
                    .map_err(err)?
                    .map(|e| (e.object_id(), e.mode())))
            };
        if entry(&a)? != entry(&b)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Files changed between `fork` and `head` (`git diff --name-only
/// --no-renames`): no renames, so a moved file counts at both paths.
fn committed_paths(
    repo: &gix::Repository,
    fork: &gf_core::model::Oid,
    head: ObjectId,
) -> Result<Vec<std::path::PathBuf>> {
    let tree = |id: ObjectId| -> Result<gix::Tree<'_>> {
        repo.find_commit(id).map_err(err)?.tree().map_err(err)
    };
    let fork = ObjectId::from_hex(fork.as_str().as_bytes()).map_err(err)?;
    let (old, new) = (tree(fork)?, tree(head)?);
    let options = gix::diff::Options::default().with_rewrites(None);
    let changes = repo
        .diff_tree_to_tree(Some(&old), Some(&new), Some(options))
        .map_err(err)?;
    Ok(changes
        .iter()
        .filter(|c| !c.entry_mode().is_tree())
        .map(|c| super::gix_status::path(c.location()))
        .collect())
}

/// See `CliBackend::default_base`.
/// See `WorktreeOps::pull_rebases`.
pub(super) fn pull_rebases(path: &Path, branch: &str) -> Result<bool> {
    let repo = gix::open(path).map_err(err)?;
    let config = repo.config_snapshot();
    let read = |section: &str, sub: Option<&str>| {
        config
            .plumbing()
            .string_by(section, sub.map(|s| s.as_bytes().as_bstr()), "rebase")
            .map(|v| text(v.as_ref()).trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let value = read("branch", Some(branch)).or_else(|| read("pull", None));
    Ok(value.as_deref().is_some_and(crate::ops::worktree::rebases))
}

pub(super) fn default_base(repo: &gix::Repository) -> Result<Option<String>> {
    let local = |name: &str| exists(repo, &format!("refs/heads/{name}")).then(|| name.to_owned());
    if let Some(r) = repo
        .try_find_reference("refs/remotes/origin/HEAD")
        .map_err(err)?
        && let TargetRef::Symbolic(target) = r.target()
    {
        let remote_head = text(target.shorten());
        if let Some((_, short)) = remote_head.split_once('/')
            && let Some(name) = local(short)
        {
            return Ok(Some(name));
        }
        if !remote_head.is_empty() {
            return Ok(Some(remote_head));
        }
    }
    Ok(["main", "master"].into_iter().find_map(local))
}
