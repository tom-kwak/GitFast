//! `git fetch --prune` through gix (feature `gix-fetch`, used only when the
//! `git.gix_fetch` setting is on). Credentials come from the same helpers
//! git uses, SSH through the `ssh` program, HTTPS through the system TLS.
//!
//! The git CLI is used instead for what this does not model: shallow
//! repositories, submodules (git fetches them on demand), refspecs other
//! than `src/*:dst/*`, remotes skipped by `--all`, and fetching a URL that
//! is not a configured remote. Unlike git it writes no `FETCH_HEAD`, runs no
//! `gc --auto`, and its reflog messages for remote-tracking refs read
//! differently; pulls use the CLI, which fetches again.
//!
//! gix has no `--prune`: after a fetch, remote-tracking refs under each
//! refspec's destination that the remote no longer has are deleted with a
//! ref transaction. Like git 2.48+ (`remote.<name>.followRemoteHEAD`,
//! default `create`), a missing `refs/remotes/<name>/HEAD` is created from
//! the remote's HEAD — only when the installed git does that too.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gf_core::{GfError, Result};
use gix::bstr::{BString, ByteSlice};
use gix::remote::Direction;
use gix::remote::fetch::refs::update::Mode;

use super::gix_backend::{err, text, unsupported};
use super::{GixBackend, SyncOps};
use crate::cli::{Progress, RunOptions};
use crate::ops::sync::{PullMode, PushOptions};

type Root = Arc<gix::progress::tree::Root>;

/// Remote-tracking prefixes `--prune` may clean, one per fetch refspec
/// (`refs/heads/*:refs/remotes/origin/*` → `refs/remotes/origin/`).
fn prune_prefixes(remote: &gix::Remote<'_>) -> Result<Vec<String>> {
    let mut prefixes = Vec::new();
    for spec in remote.refspecs(Direction::Fetch) {
        let spec = spec.to_ref();
        let (Some(src), Some(dst)) = (spec.source(), spec.destination()) else {
            return unsupported("refspec without both sides");
        };
        if !src.ends_with(b"/*")
            || !dst.ends_with(b"/*")
            || src[..src.len() - 1].contains(&b'*')
            || dst[..dst.len() - 1].contains(&b'*')
        {
            return unsupported("refspec other than src/*:dst/*");
        }
        prefixes.push(text(dst[..dst.len() - 1].as_bstr()));
    }
    Ok(prefixes)
}

/// Whether the installed git (2.48+) creates `refs/remotes/<name>/HEAD`
/// on fetch. Asked once per run.
fn git_follows_remote_head(git: &std::path::Path) -> bool {
    static FOLLOWS: OnceLock<bool> = OnceLock::new();
    *FOLLOWS.get_or_init(|| {
        let Ok(out) = std::process::Command::new(git)
            .env("PATH", gf_core::tool_path())
            .arg("--version")
            .output()
        else {
            return false;
        };
        // `git version 2.50.1 (Apple Git-155)`
        let text = String::from_utf8_lossy(&out.stdout);
        let mut nums = text
            .split_whitespace()
            .nth(2)
            .unwrap_or_default()
            .split('.')
            .map(|n| n.parse::<u32>().unwrap_or(0));
        let (major, minor) = (nums.next().unwrap_or(0), nums.next().unwrap_or(0));
        (major, minor) >= (2, 48)
    })
}

/// `git fetch` creating a missing `refs/remotes/<name>/HEAD` from the
/// remote's HEAD (`followRemoteHEAD` = `create` or `warn`).
fn create_remote_head(
    repo: &gix::Repository,
    name: &str,
    ref_map: &gix::remote::fetch::RefMap,
) -> Result<()> {
    let config = repo.config_snapshot();
    match config
        .string(format!("remote.{name}.followRemoteHEAD").as_str())
        .map(|v| v.to_string())
        .as_deref()
    {
        None | Some("create" | "warn") => {}
        Some("never") => return Ok(()),
        Some(_) => return unsupported("remote.<name>.followRemoteHEAD"),
    }
    let head = format!("refs/remotes/{name}/HEAD");
    if repo
        .try_find_reference(head.as_str())
        .map_err(err)?
        .is_some()
    {
        return Ok(());
    }
    let target = ref_map.remote_refs.iter().find_map(|r| match r {
        gix::protocol::handshake::Ref::Symbolic {
            full_ref_name,
            target,
            ..
        } if full_ref_name == "HEAD" => Some(target.clone()),
        _ => None,
    });
    let local = target.and_then(|t| {
        ref_map
            .mappings
            .iter()
            .find(|m| m.remote.as_name() == Some(t.as_bstr()))
            .and_then(|m| m.local.clone())
    });
    let Some(local) = local else {
        return Ok(());
    };
    repo.edit_reference(gix::refs::transaction::RefEdit {
        change: gix::refs::transaction::Change::Update {
            log: gix::refs::transaction::LogChange {
                mode: gix::refs::transaction::RefLog::AndReference,
                force_create_reflog: false,
                message: "fetch: remote HEAD".into(),
            },
            expected: gix::refs::transaction::PreviousValue::MustNotExist,
            new: gix::refs::Target::Symbolic(local.try_into().map_err(err)?),
        },
        name: head.as_str().try_into().map_err(err)?,
        deref: false,
    })
    .map_err(err)?;
    Ok(())
}

/// Like git, also ask for `HEAD` when the remote HEAD is followed (the
/// server only lists refs matching the requested prefixes).
fn ref_map_options(follow_head: bool) -> Result<gix::remote::ref_map::Options> {
    let mut options = gix::remote::ref_map::Options::default();
    if follow_head {
        let head = gix::refspec::parse("HEAD".into(), gix::refspec::parse::Operation::Fetch)
            .map_err(err)?
            .to_owned();
        options.extra_refspecs.push(head);
    }
    Ok(options)
}

/// Remotes `git fetch --all` would fetch, in name order.
fn all_remotes(repo: &gix::Repository) -> Result<Vec<String>> {
    let config = repo.config_snapshot();
    let mut names = Vec::new();
    for name in repo.remote_names() {
        let name = text(name.as_ref());
        for key in ["skipFetchAll", "skipDefaultUpdate"] {
            if config
                .boolean(format!("remote.{name}.{key}").as_str())
                .unwrap_or(false)
            {
                return unsupported("remote skipped by --all");
            }
        }
        names.push(name);
    }
    Ok(names)
}

/// Fetches and prunes; returns git-like summary lines.
fn fetch(
    repo: &gix::Repository,
    remote: Option<&str>,
    follow_head: bool,
    root: &Root,
    interrupt: &AtomicBool,
) -> Result<Vec<String>> {
    if repo.is_shallow().map_err(err)? {
        return unsupported("fetch into a shallow repository");
    }
    if repo
        .workdir()
        .is_some_and(|w| w.join(".gitmodules").exists())
    {
        return unsupported("fetch with submodules");
    }
    let names = match remote {
        Some(name) => {
            if !repo
                .remote_names()
                .iter()
                .any(|n| n.as_bytes() == name.as_bytes())
            {
                return unsupported("fetch from a URL");
            }
            vec![name.to_owned()]
        }
        None => all_remotes(repo)?,
    };

    let mut lines = Vec::new();
    for name in names {
        let remote = repo.find_remote(name.as_str()).map_err(err)?;
        let prefixes = prune_prefixes(&remote)?;
        let outcome = remote
            .connect(Direction::Fetch)
            .map_err(err)?
            .prepare_fetch(gix::progress::Discard, ref_map_options(follow_head)?)
            .map_err(err)?
            .receive(root.add_child(format!("fetch {name}")), interrupt)
            .map_err(err)?;

        let update_refs = match &outcome.status {
            gix::remote::fetch::Status::NoPackReceived { update_refs, .. }
            | gix::remote::fetch::Status::Change { update_refs, .. } => update_refs,
        };
        let (mut new, mut updated) = (0, 0);
        for update in &update_refs.updates {
            match update.mode {
                Mode::New => new += 1,
                Mode::FastForward | Mode::Forced => updated += 1,
                Mode::NoChangeNeeded | Mode::ImplicitTagNotSentByRemote => {}
                _ => {
                    return Err(err(format!(
                        "{name}: a ref could not be updated: {:?}",
                        update.mode
                    )));
                }
            }
        }

        if follow_head {
            create_remote_head(repo, &name, &outcome.ref_map)?;
        }

        // --prune: tracking refs the remote no longer has.
        let keep: HashSet<BString> = outcome
            .ref_map
            .mappings
            .iter()
            .filter_map(|m| m.local.clone())
            .collect();
        let mut pruned = 0;
        for prefix in &prefixes {
            let stale: Vec<(String, gix::ObjectId)> = repo
                .references()
                .map_err(err)?
                .prefixed(prefix.as_str())
                .map_err(err)?
                .filter_map(std::result::Result::ok)
                .filter_map(|r| match r.target() {
                    gix::refs::TargetRef::Object(id) => {
                        Some((text(r.name().as_bstr()), id.to_owned()))
                    }
                    gix::refs::TargetRef::Symbolic(_) => None,
                })
                .filter(|(full, _)| !keep.contains(full.as_bytes().as_bstr()))
                .collect();
            for (full, id) in stale {
                repo.edit_reference(gix::refs::transaction::RefEdit {
                    change: gix::refs::transaction::Change::Delete {
                        expected: gix::refs::transaction::PreviousValue::MustExistAndMatch(
                            gix::refs::Target::Object(id),
                        ),
                        log: gix::refs::transaction::RefLog::AndReference,
                    },
                    name: full.as_str().try_into().map_err(err)?,
                    deref: false,
                })
                .map_err(err)?;
                lines.push(format!(
                    " - [deleted]  {}",
                    full.trim_start_matches("refs/remotes/")
                ));
                pruned += 1;
            }
        }
        lines.push(format!(
            "{name}: {new} new, {updated} updated, {pruned} pruned"
        ));
    }
    Ok(lines)
}

/// The step of the most recent task that has a known total.
fn progress_of<K>(snapshot: &[(K, gix::progress::Task)]) -> Option<Progress> {
    snapshot.iter().rev().find_map(|(_, task)| {
        let value = task.progress.as_ref()?;
        let total = value.done_at?;
        let current = value.step.load(Ordering::Relaxed);
        let percent = (current.min(total) * 100)
            .checked_div(total)
            .map_or(100, |p| u8::try_from(p).unwrap_or(100));
        Some(Progress {
            phase: task.name.clone(),
            remote: false,
            percent,
            current: current as u64,
            total: total as u64,
        })
    })
}

impl SyncOps for GixBackend {
    async fn fetch(&self, remote: Option<&str>, opts: RunOptions) -> Result<()> {
        let started = Instant::now();
        let cancelled = || opts.cancel.as_ref().is_some_and(|t| t.is_cancelled());
        if cancelled() {
            return Err(GfError::Cancelled);
        }
        let interrupt = Arc::new(AtomicBool::new(false));
        let root: Root = gix::progress::tree::Root::new();

        let mut helpers = Vec::new();
        if let Some(token) = opts.cancel.clone() {
            let flag = interrupt.clone();
            helpers.push(tokio::spawn(async move {
                token.cancelled().await;
                flag.store(true, Ordering::Relaxed);
            }));
        }
        if let Some(tx) = opts.progress.clone() {
            let root = root.clone();
            helpers.push(tokio::spawn(async move {
                let mut snapshot = Vec::new();
                let mut last = None;
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    root.sorted_snapshot(&mut snapshot);
                    let now = progress_of(&snapshot);
                    if now.is_some() && now != last {
                        if tx.send(now.clone().expect("checked")).is_err() {
                            break;
                        }
                        last = now;
                    }
                }
            }));
        }

        let name = remote.map(str::to_owned);
        let flag = interrupt.clone();
        let tree = root.clone();
        let git = self.runner.git_executable().to_path_buf();
        let result = self
            .blocking(move |repo| {
                let follow_head = git_follows_remote_head(&git);
                fetch(repo, name.as_deref(), follow_head, &tree, &flag)
            })
            .await;
        for h in helpers {
            h.abort();
        }
        if interrupt.load(Ordering::Relaxed) || cancelled() {
            return Err(GfError::Cancelled);
        }
        let lines = result?;
        let mut argv: Vec<String> = ["gix", "fetch", "--prune"].map(str::to_owned).into();
        argv.push(remote.map_or_else(|| "--all".to_owned(), str::to_owned));
        self.runner
            .report_in_process(Some(self.repo_id), argv, &lines, 0, started.elapsed());
        Ok(())
    }

    async fn pull(&self, _mode: PullMode, _opts: RunOptions) -> Result<()> {
        unsupported("pull")
    }

    async fn push(&self, _push: &PushOptions, _opts: RunOptions) -> Result<()> {
        unsupported("push")
    }
}
