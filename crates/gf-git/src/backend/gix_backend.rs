//! [`GixBackend`]: in-process reads through gitoxide (gix 0.90, stable
//! APIs only). It implements the read side of some interfaces; everything
//! else returns [`GfError::Unsupported`] and `routes.rs` uses the git CLI.
//! Results must equal the CLI's — `tests/gix_contract.rs` checks that.
//!
//! When in doubt (a feature this code does not model, like URL rewrites or
//! signed tags) a method answers `Unsupported` rather than risk a result
//! that differs from git.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use gf_core::model::{
    Branch, Commit, Oid, RefName, Remote, RepoId, Signature, StashEntry, Tag, Timestamp, Upstream,
};
use gf_core::{GfError, Result};
use gix::ObjectId;
use gix::bstr::{BStr, ByteSlice};
use gix::refs::TargetRef;
use gix::revision::walk::Sorting;
use gix::traverse::commit::Parents;
use gix::traverse::commit::simple::CommitTimeOrder;
use gix::traverse::commit::topo;

use super::{LogOps, RebaseOps, RefOps, ReflogOps, RemoteOps, StashOps, WorktreeOps};
use crate::cli::GitRunner;
use crate::cli::parse::log::split_message;
use crate::ops::integrate::Integration;
use crate::ops::log::{LogOrder, LogQuery};
use crate::ops::rebase_interactive::{self, SquashPreview, TodoItem};
use crate::ops::reflog::{ReflogEntry, ResetMode};
use crate::ops::stash::StashOptions;
use crate::ops::worktree::Worktree;

/// A repository opened with gix. Cheap to clone; reopened when the
/// repository's config file changes (remotes, upstreams) so reads never
/// use stale configuration.
#[derive(Debug, Clone)]
pub struct GixBackend {
    inner: Arc<Mutex<Opened>>,
    /// Reports writes to the command log like git commands.
    pub(super) runner: GitRunner,
    pub(super) repo_id: RepoId,
}

#[derive(Debug)]
struct Opened {
    repo: gix::ThreadSafeRepository,
    config: PathBuf,
    stamp: Option<SystemTime>,
}

pub(super) fn err(e: impl std::fmt::Display) -> GfError {
    GfError::Other(format!("gix: {e}"))
}

pub(super) fn unsupported<T>(what: &'static str) -> Result<T> {
    Err(GfError::Unsupported(what))
}

fn modified(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl GixBackend {
    /// `None` when gix cannot open the repository; the CLI is used then.
    pub fn open(git_dir: &std::path::Path, runner: GitRunner, repo_id: RepoId) -> Option<Self> {
        match Opened::open(git_dir) {
            Ok(opened) => Some(Self {
                inner: Arc::new(Mutex::new(opened)),
                runner,
                repo_id,
            }),
            Err(e) => {
                tracing::debug!(error = %e, "gix could not open repository; CLI only");
                None
            }
        }
    }

    fn repo(&self) -> Result<gix::ThreadSafeRepository> {
        let mut opened = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if modified(&opened.config) != opened.stamp {
            let git_dir = opened.repo.git_dir().to_path_buf();
            *opened = Opened::open(&git_dir)?;
        }
        Ok(opened.repo.clone())
    }

    /// Runs `f` on a blocking thread (gix is synchronous).
    pub(super) async fn blocking<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&gix::Repository) -> Result<T> + Send + 'static,
    {
        let repo = self.repo()?;
        tokio::task::spawn_blocking(move || f(&repo.to_thread_local()))
            .await
            .map_err(|e| err(format!("task: {e}")))?
    }
}

impl Opened {
    fn open(git_dir: &std::path::Path) -> Result<Self> {
        let mut local = gix::ThreadSafeRepository::open(git_dir)
            .map_err(err)?
            .to_thread_local();
        // Ref edits write a reflog entry, which needs a committer. Without
        // user.name/email (a new Mac) git falls back to the login name;
        // gix refuses instead, so give it the same kind of fallback.
        let user = std::env::var("USER").unwrap_or_else(|_| "gf".into());
        local
            .committer_or_set_fallback(user.as_str(), format!("{user}@localhost").as_str())
            .map_err(err)?;
        let config = local.common_dir().join("config");
        let repo = local.into_sync();
        Ok(Self {
            stamp: modified(&config),
            repo,
            config,
        })
    }
}

// ---------------------------------------------------------------- helpers

pub(super) fn oid(id: impl Into<ObjectId>) -> Oid {
    // gix renders full lowercase hex, which `Oid` accepts.
    id.into()
        .to_string()
        .parse()
        .unwrap_or_else(|_| unreachable!("gix produced an invalid object id"))
}

fn object_id(id: &Oid) -> Result<ObjectId> {
    ObjectId::from_hex(id.as_str().as_bytes()).map_err(err)
}

fn signature(sig: gix::actor::SignatureRef<'_>) -> Result<Signature> {
    let time = sig.time().map_err(err)?;
    Ok(Signature {
        name: String::from_utf8_lossy(sig.name).trim().to_owned(),
        email: String::from_utf8_lossy(sig.email).trim().to_owned(),
        time: Timestamp {
            seconds: time.seconds,
            offset_minutes: time.offset / 60,
        },
    })
}

fn commit(repo: &gix::Repository, id: ObjectId) -> Result<Commit> {
    let c = repo.find_commit(id).map_err(err)?;
    let message = c.message_raw().map_err(err)?;
    let (summary, body) = split_message(&String::from_utf8_lossy(message));
    Ok(Commit {
        id: oid(id),
        parents: c.parent_ids().map(|p| oid(p.detach())).collect(),
        author: signature(c.author().map_err(err)?)?,
        committer: signature(c.committer().map_err(err)?)?,
        summary,
        body,
    })
}

/// HEAD's commit; `None` on an unborn branch.
pub(super) fn head_id(repo: &gix::Repository) -> Result<Option<ObjectId>> {
    Ok(repo.head().map_err(err)?.id().map(gix::Id::detach))
}

/// A revision as a commit id (tags are peeled).
pub(super) fn resolve(repo: &gix::Repository, rev: &str) -> Result<ObjectId> {
    let id = repo.rev_parse_single(rev).map_err(err)?;
    Ok(id.object().map_err(err)?.peel_to_commit().map_err(err)?.id)
}

/// Commit ids every ref under `prefix` (and HEAD, with `head`) points to.
/// Symbolic refs and refs to non-commits are skipped.
fn ref_tips(repo: &gix::Repository, prefix: Option<&str>) -> Result<Vec<ObjectId>> {
    let platform = repo.references().map_err(err)?;
    let iter = match prefix {
        Some(p) => platform.prefixed(p).map_err(err)?,
        None => platform.all().map_err(err)?,
    };
    let mut tips = Vec::new();
    for r in iter {
        let mut r = r.map_err(err)?;
        if matches!(r.target(), TargetRef::Symbolic(_)) {
            continue;
        }
        let id = r.peel_to_id().map_err(err)?.detach();
        if repo.find_commit(id).is_ok() {
            tips.push(id);
        }
    }
    Ok(tips)
}

fn count(repo: &gix::Repository, tip: ObjectId, hidden: ObjectId) -> Result<u32> {
    let mut n = 0u32;
    for info in repo
        .rev_walk([tip])
        .with_hidden([hidden])
        .all()
        .map_err(err)?
    {
        info.map_err(err)?;
        n += 1;
    }
    Ok(n)
}

fn walk_log(repo: &gix::Repository, q: &LogQuery) -> Result<Vec<Commit>> {
    let mut tips = Vec::new();
    let mut hidden = Vec::new();
    if q.all_refs {
        tips.extend(head_id(repo)?);
        tips.extend(ref_tips(repo, None)?);
    }
    for rev in &q.revisions {
        if rev.starts_with('-') || rev.contains("...") {
            return unsupported("log revision syntax");
        }
        if let Some((from, to)) = rev.split_once("..") {
            let side = |s: &str| if s.is_empty() { "HEAD" } else { s }.to_owned();
            hidden.push(resolve(repo, &side(from))?);
            tips.push(resolve(repo, &side(to))?);
        } else if let Some(excluded) = rev.strip_prefix('^') {
            hidden.push(resolve(repo, excluded)?);
        } else {
            tips.push(resolve(repo, rev)?);
        }
    }
    if q.revisions.is_empty() && !q.all_refs {
        match head_id(repo)? {
            Some(id) => tips.push(id),
            None => return Ok(Vec::new()),
        }
    }
    let mut seen = std::collections::HashSet::new();
    tips.retain(|t| seen.insert(*t));

    let max = q.max_count.unwrap_or(usize::MAX);
    let ids: Vec<ObjectId> = match q.order {
        LogOrder::Default => {
            let mut walk = repo
                .rev_walk(tips)
                .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst))
                .with_hidden(hidden);
            if q.first_parent {
                walk = walk.first_parent_only();
            }
            walk.all()
                .map_err(err)?
                .skip(q.skip)
                .take(max)
                .map(|i| i.map(|i| i.id).map_err(err))
                .collect::<Result<_>>()?
        }
        LogOrder::Topo | LogOrder::Date => {
            let sorting = if q.order == LogOrder::Topo {
                topo::Sorting::TopoOrder
            } else {
                topo::Sorting::DateOrder
            };
            let parents = if q.first_parent {
                Parents::First
            } else {
                Parents::All
            };
            // The topo walk does not follow shallow boundaries.
            if repo.is_shallow().map_err(err)? {
                return unsupported("topo walk in a shallow repository");
            }
            // With a commit-graph, generation numbers let gix stop early and
            // it beats git; without one git is ~3x faster (measured with
            // `tests/gix_bench.rs`), so leave it to git.
            let Some(graph) = repo.commit_graph_if_enabled().map_err(err)? else {
                return unsupported("topo walk without a commit-graph");
            };
            topo::Builder::from_iters(&repo.objects, tips, Some(hidden))
                .sorting(sorting)
                .parents(parents)
                .with_commit_graph(Some(graph))
                .build()
                .map_err(err)?
                .skip(q.skip)
                .take(max)
                .map(|i| i.map(|i| i.id).map_err(err))
                .collect::<Result<_>>()?
        }
    };
    ids.into_iter().map(|id| commit(repo, id)).collect()
}

/// `branch.<short>.remote` / `.merge` mapped to the remote-tracking ref,
/// like `%(upstream)`; `None` when not configured.
fn upstream_name(repo: &gix::Repository, full: &gix::refs::FullNameRef) -> Result<Option<String>> {
    let short = full.shorten();
    let config = repo.config_snapshot();
    let file = config.plumbing();
    let get = |key: &str| file.string_by("branch", Some(short), key);
    let (Some(remote), Some(merge)) = (get("remote"), get("merge")) else {
        return Ok(None);
    };
    if remote.as_bytes() == b"." {
        return Ok(Some(merge.to_string()));
    }
    match repo.branch_remote_tracking_ref_name(full, gix::remote::Direction::Fetch) {
        None => Ok(None),
        Some(name) => Ok(Some(name.map_err(err)?.as_bstr().to_string())),
    }
}

pub(super) fn upstream(
    repo: &gix::Repository,
    full: &gix::refs::FullNameRef,
    tip: ObjectId,
) -> Result<Option<Upstream>> {
    let Some(name) = upstream_name(repo, full)? else {
        return Ok(None);
    };
    let found = repo.try_find_reference(name.as_str()).map_err(err)?;
    let Some(mut reference) = found else {
        return Ok(Some(Upstream {
            name: RefName::new(name),
            ahead: 0,
            behind: 0,
            gone: true,
        }));
    };
    let up = reference.peel_to_id().map_err(err)?.detach();
    Ok(Some(Upstream {
        name: RefName::new(name),
        ahead: count(repo, tip, up)?,
        behind: count(repo, up, tip)?,
        gone: false,
    }))
}

/// Commits reachable from HEAD that are on no remote (and not reachable
/// from `since`), newest first.
fn unpushed(repo: &gix::Repository, since: Option<ObjectId>) -> Result<Vec<ObjectId>> {
    unpushed_limited(repo, since, usize::MAX)
}

fn unpushed_limited(
    repo: &gix::Repository,
    since: Option<ObjectId>,
    limit: usize,
) -> Result<Vec<ObjectId>> {
    let Some(head) = head_id(repo)? else {
        return Ok(Vec::new());
    };
    let mut hidden = ref_tips(repo, Some("refs/remotes/"))?;
    hidden.extend(since);
    repo.rev_walk([head])
        .with_hidden(hidden)
        .all()
        .map_err(err)?
        .take(limit)
        .map(|i| i.map(|i| i.id).map_err(err))
        .collect()
}

/// Newest-first reflog lines of `name` (`HEAD`, `refs/stash`).
fn reflog_lines(
    repo: &gix::Repository,
    name: &str,
    limit: usize,
) -> Result<Vec<gix::refs::log::Line>> {
    let Some(reference) = repo.try_find_reference(name).map_err(err)? else {
        return Ok(Vec::new());
    };
    let mut platform = reference.log_iter();
    let Some(lines) = platform.rev().map_err(err)? else {
        return Ok(Vec::new());
    };
    lines
        .take(limit)
        .map(|l| l.map_err(err))
        .collect::<Result<_>>()
}

pub(super) fn text(s: &BStr) -> String {
    s.to_str_lossy().into_owned()
}

// ------------------------------------------------------------- interfaces

impl LogOps for GixBackend {
    async fn log(&self, query: &LogQuery) -> Result<Vec<Commit>> {
        if !query.paths.is_empty()
            || query.follow
            || query.author.is_some()
            || query.grep.is_some()
            || query.pickaxe.is_some()
        {
            return unsupported("log filters");
        }
        let q = query.clone();
        self.blocking(move |repo| walk_log(repo, &q)).await
    }

    async fn head_exists(&self) -> Result<bool> {
        self.blocking(|repo| Ok(head_id(repo)?.is_some())).await
    }
}

impl RefOps for GixBackend {
    async fn branches(&self) -> Result<Vec<Branch>> {
        self.blocking(|repo| {
            let head = repo.head().map_err(err)?;
            let current = head.referent_name().map(|n| n.as_bstr().to_owned());
            let platform = repo.references().map_err(err)?;
            let mut branches = Vec::new();
            for prefix in ["refs/heads/", "refs/remotes/"] {
                for r in platform.prefixed(prefix).map_err(err)? {
                    let r = r.map_err(err)?;
                    let TargetRef::Object(id) = r.target() else {
                        continue; // `origin/HEAD` points to a branch.
                    };
                    let id = id.to_owned();
                    let full = r.name();
                    let upstream = if prefix == "refs/heads/" {
                        upstream(repo, full, id)?
                    } else {
                        None
                    };
                    branches.push(Branch {
                        name: RefName::new(text(full.as_bstr())),
                        target: oid(id),
                        upstream,
                        is_head: current
                            .as_ref()
                            .is_some_and(|c| c.as_bstr() == full.as_bstr()),
                    });
                }
            }
            branches.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
            Ok(branches)
        })
        .await
    }

    async fn tags(&self) -> Result<Vec<Tag>> {
        self.blocking(|repo| {
            let platform = repo.references().map_err(err)?;
            let mut tags = Vec::new();
            for r in platform.prefixed("refs/tags/").map_err(err)? {
                let r = r.map_err(err)?;
                let TargetRef::Object(id) = r.target() else {
                    return unsupported("symbolic tag ref");
                };
                let id = id.to_owned();
                let object = repo.find_object(id).map_err(err)?;
                let (target, message) = if object.kind == gix::object::Kind::Tag {
                    let tag = object.into_tag();
                    let decoded = tag.decode().map_err(err)?;
                    if decoded.signature.is_some() {
                        return unsupported("signed tag");
                    }
                    (
                        decoded.target(),
                        Some(text(decoded.message).trim_end().to_owned()),
                    )
                } else {
                    (id, None)
                };
                tags.push(Tag {
                    name: RefName::new(text(r.name().as_bstr())),
                    target: oid(target),
                    message,
                });
            }
            tags.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
            Ok(tags)
        })
        .await
    }

    async fn unpushed_count(&self) -> Result<u32> {
        self.blocking(|repo| u32::try_from(unpushed(repo, None)?.len()).map_err(err))
            .await
    }

    async fn unpushed(&self, since: Option<&Oid>) -> Result<Vec<Oid>> {
        let since = since.map(object_id).transpose()?;
        self.blocking(move |repo| Ok(unpushed(repo, since)?.into_iter().map(oid).collect()))
            .await
    }

    async fn unpushed_recent(&self, limit: usize) -> Result<Vec<Oid>> {
        self.blocking(move |repo| {
            Ok(unpushed_limited(repo, None, limit)?
                .into_iter()
                .map(oid)
                .collect())
        })
        .await
    }
}

impl RemoteOps for GixBackend {
    async fn remotes(&self) -> Result<Vec<Remote>> {
        self.blocking(|repo| {
            let config = repo.config_snapshot();
            let file = config.plumbing();
            // git rewrites URLs with `url.<base>.insteadOf`; not modelled.
            if file
                .sections_by_name("url")
                .is_some_and(|mut s| s.next().is_some())
            {
                return unsupported("url rewrites");
            }
            let mut remotes = Vec::new();
            for name in repo.remote_names() {
                let name: &BStr = name.as_ref();
                let urls = file
                    .strings_by("remote", Some(name), "url")
                    .unwrap_or_default();
                let [url] = urls.as_slice() else {
                    return unsupported("remote without exactly one url");
                };
                let pushes = file
                    .strings_by("remote", Some(name), "pushurl")
                    .unwrap_or_default();
                let push_url = match pushes.as_slice() {
                    [] => None,
                    [p] => (p != url).then(|| text(p.as_ref())),
                    _ => return unsupported("several push urls"),
                };
                remotes.push(Remote {
                    name: text(name),
                    fetch_refspecs: file
                        .strings_by("remote", Some(name), "fetch")
                        .unwrap_or_default()
                        .iter()
                        .map(|spec| text(spec.as_ref()))
                        .collect(),
                    fetch_url: text(url.as_ref()),
                    push_url,
                });
            }
            Ok(remotes)
        })
        .await
    }
}

impl ReflogOps for GixBackend {
    async fn reflog(&self, limit: usize) -> Result<Vec<ReflogEntry>> {
        self.blocking(move |repo| {
            Ok(reflog_lines(repo, "HEAD", limit)?
                .into_iter()
                .enumerate()
                .map(|(i, line)| ReflogEntry {
                    id: oid(line.new_oid),
                    selector: format!("HEAD@{{{i}}}"),
                    message: text(line.message.as_ref()),
                })
                .collect())
        })
        .await
    }

    async fn head_oid(&self) -> Result<Option<Oid>> {
        self.blocking(|repo| Ok(head_id(repo)?.map(oid))).await
    }

    async fn reset(&self, _to: &Oid, _mode: ResetMode) -> Result<()> {
        unsupported("reset")
    }

    async fn undo_to(&self, _to: &Oid, _soft: bool) -> Result<()> {
        unsupported("undo")
    }
}

impl StashOps for GixBackend {
    async fn stashes(&self) -> Result<Vec<StashEntry>> {
        self.blocking(|repo| {
            reflog_lines(repo, "refs/stash", usize::MAX)?
                .into_iter()
                .enumerate()
                .map(|(index, line)| {
                    let c = repo.find_commit(line.new_oid).map_err(err)?;
                    let seconds = c.committer().map_err(err)?.time().map_err(err)?.seconds;
                    Ok(StashEntry {
                        index,
                        id: oid(line.new_oid),
                        message: text(line.message.as_ref()),
                        time: Timestamp {
                            seconds,
                            offset_minutes: 0,
                        },
                    })
                })
                .collect()
        })
        .await
    }

    async fn stash_push(&self, _opts: &StashOptions) -> Result<()> {
        unsupported("stash push")
    }

    async fn stash_apply(&self, _index: usize) -> Result<()> {
        unsupported("stash apply")
    }

    async fn stash_pop(&self, _index: usize) -> Result<()> {
        unsupported("stash pop")
    }

    async fn stash_drop(&self, _index: usize) -> Result<()> {
        unsupported("stash drop")
    }
}

impl WorktreeOps for GixBackend {
    async fn worktrees(&self) -> Result<Vec<Worktree>> {
        self.blocking(|repo| {
            let entry = |r: &gix::Repository, path: PathBuf| -> Result<Worktree> {
                let head = r.head().map_err(err)?;
                if head.is_unborn() {
                    return unsupported("worktree on an unborn branch");
                }
                Ok(Worktree {
                    path,
                    head: head.id().map(|i| oid(i.detach())),
                    branch: head.referent_name().map(|n| text(n.shorten())),
                    bare: false,
                    locked: false,
                    prunable: false,
                })
            };
            let mut list = Vec::new();
            match repo.workdir() {
                Some(dir) => list.push(entry(repo, dir.to_path_buf())?),
                None => list.push(Worktree {
                    path: repo.git_dir().to_path_buf(),
                    head: None,
                    branch: None,
                    bare: true,
                    locked: false,
                    prunable: false,
                }),
            }
            for proxy in repo.worktrees().map_err(err)? {
                let path = proxy.base().map_err(err)?;
                let locked = proxy.is_locked();
                let prunable = !path.exists();
                let linked = proxy
                    .into_repo_with_possibly_inaccessible_worktree()
                    .map_err(err)?;
                list.push(Worktree {
                    locked,
                    prunable,
                    ..entry(&linked, path)?
                });
            }
            Ok(list)
        })
        .await
    }

    async fn add_worktree(&self, _: &std::path::Path, _: &str, _: bool) -> Result<()> {
        unsupported("worktree add")
    }

    async fn remove_worktree(&self, _: &std::path::Path, _: bool) -> Result<()> {
        unsupported("worktree remove")
    }

    async fn prune_worktrees(&self) -> Result<()> {
        unsupported("worktree prune")
    }
}

impl RebaseOps for GixBackend {
    async fn rebase_plan(&self, base: Option<&Oid>) -> Result<Vec<TodoItem>> {
        rebase_interactive::plan(self, base).await
    }

    async fn squash_preview(&self, ids: &[Oid]) -> Result<SquashPreview> {
        rebase_interactive::preview(self, ids).await
    }

    async fn squash(&self, _: &[Oid], _: &str, _: bool) -> Result<Integration> {
        unsupported("squash")
    }

    async fn rebase_interactive(&self, _: Option<&Oid>, _: &[TodoItem]) -> Result<Integration> {
        unsupported("interactive rebase")
    }
}
