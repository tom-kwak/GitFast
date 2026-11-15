//! In-process reads via gitoxide, used as a fast path for hot queries.
//! Every method has a CLI equivalent that is used when this one fails or
//! the query needs features implemented only by git itself.

use std::path::Path;

use gf_core::model::{Commit, Oid, Signature, Timestamp};
use gf_core::{GfError, Result};
use gix::revision::walk::Sorting;
use gix::traverse::commit::simple::CommitTimeOrder;

use crate::cli::parse::log::split_message;
use crate::ops::log::LogQuery;

#[derive(Debug, Clone)]
pub(crate) struct GixReader {
    repo: gix::ThreadSafeRepository,
}

fn err(e: impl std::fmt::Display) -> GfError {
    GfError::Other(format!("gix: {e}"))
}

impl GixReader {
    /// `None` when gix cannot open the repository; callers then use the CLI.
    pub(crate) fn open(git_dir: &Path) -> Option<Self> {
        match gix::ThreadSafeRepository::open(git_dir) {
            Ok(repo) => Some(Self { repo }),
            Err(e) => {
                tracing::debug!(error = %e, "gix could not open repository; CLI only");
                None
            }
        }
    }

    /// Equivalent of `git log` (default order) for a [`LogQuery`] where
    /// [`LogQuery::is_plain_walk`] is true.
    pub(crate) fn log(&self, q: &LogQuery) -> Result<Vec<Commit>> {
        let repo = self.repo.to_thread_local();

        let tips = if q.revisions.is_empty() {
            let head = repo.head().map_err(err)?;
            if head.is_unborn() {
                return Ok(Vec::new());
            }
            vec![head.id().ok_or_else(|| err("HEAD has no id"))?.detach()]
        } else {
            q.revisions
                .iter()
                .map(|r| {
                    let id = repo.rev_parse_single(r.as_str()).map_err(err)?;
                    // Tags must be peeled to the commit they point to.
                    Ok(id.object().map_err(err)?.peel_to_commit().map_err(err)?.id)
                })
                .collect::<Result<Vec<_>>>()?
        };

        let walk = repo
            .rev_walk(tips)
            .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst))
            .all()
            .map_err(err)?;

        walk.skip(q.skip)
            .take(q.max_count.unwrap_or(usize::MAX))
            .map(|info| {
                let info = info.map_err(err)?;
                let commit = info.object().map_err(err)?;
                let message = commit.message_raw().map_err(err)?;
                let (summary, body) = split_message(&String::from_utf8_lossy(message));
                Ok(Commit {
                    id: oid(info.id),
                    parents: commit.parent_ids().map(|p| oid(p.detach())).collect(),
                    author: signature(commit.author().map_err(err)?)?,
                    committer: signature(commit.committer().map_err(err)?)?,
                    summary,
                    body,
                })
            })
            .collect()
    }
}

fn oid(id: gix::ObjectId) -> Oid {
    // gix always renders full lowercase hex, which `Oid` accepts.
    id.to_string()
        .parse()
        .unwrap_or_else(|_| unreachable!("gix produced an invalid object id"))
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
