//! Branch and tag ref edits through gix (atomic ref transactions). Only
//! the cases that are pure ref edits are handled here; everything git does
//! on top (worktree updates, upstream config, hooks, annotated tags) goes to
//! the git CLI:
//!
//! - a `reference-transaction` hook is installed → CLI (gix runs no hooks);
//! - creating a branch from a remote-tracking branch or with a non-default
//!   `branch.autoSetupMerge` (git would set up tracking) → CLI;
//! - deleting a branch with config (`branch.<name>.*`, which git removes),
//!   checked out or in use anywhere, or not fully merged without force → CLI;
//! - rename, switch, annotated tags, pushes → CLI.
//!
//! On success the edit is reported to the command log as `gix …`. Any error
//! makes `routes.rs` run the git CLI, which reports git's own message.

use std::path::PathBuf;
use std::time::Instant;

use gf_core::Result;
use gix::ObjectId;
use gix::bstr::ByteSlice;
use gix::refs::transaction::{Change, PreviousValue, RefEdit, RefLog};
use gix::refs::{Target, TargetRef};

use super::gix_backend::{err, head_id, resolve, text, unsupported};
use super::{BranchOps, GixBackend, TagOps};

fn short(id: ObjectId) -> String {
    id.to_hex_with_len(7).to_string()
}

/// gix runs no hooks; leave ref edits to git when one would run.
fn no_ref_hooks(repo: &gix::Repository) -> Result<()> {
    let dir = match repo.config_snapshot().string("core.hooksPath") {
        Some(p) => {
            let p = PathBuf::from(text(p.as_ref()));
            if p.starts_with("~") {
                return unsupported("core.hooksPath with ~");
            }
            if p.is_relative() {
                repo.workdir().unwrap_or(repo.git_dir()).join(p)
            } else {
                p
            }
        }
        None => repo.common_dir().join("hooks"),
    };
    if dir.join("reference-transaction").exists() {
        return unsupported("reference-transaction hook");
    }
    Ok(())
}

/// A name git accepts for a branch or tag (fully checked again by gix).
fn plain_name(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('-') || name == "HEAD" {
        return unsupported("unusual ref name");
    }
    Ok(())
}

/// Whether `full` is checked out, or being rebased or bisected, in any
/// worktree (git refuses to delete such a branch).
fn in_use(repo: &gix::Repository, full: &str) -> Result<bool> {
    let busy = |r: &gix::Repository| -> Result<bool> {
        if r.state().is_some() {
            return Ok(true);
        }
        let head = r.head().map_err(err)?;
        Ok(head
            .referent_name()
            .is_some_and(|n| n.as_bstr() == full.as_bytes().as_bstr()))
    };
    if busy(repo)? {
        return Ok(true);
    }
    for proxy in repo.worktrees().map_err(err)? {
        let linked = proxy
            .into_repo_with_possibly_inaccessible_worktree()
            .map_err(err)?;
        if busy(&linked)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn delete_ref(repo: &gix::Repository, full: &str, tip: ObjectId) -> Result<()> {
    repo.edit_reference(RefEdit {
        change: Change::Delete {
            expected: PreviousValue::MustExistAndMatch(Target::Object(tip)),
            log: RefLog::AndReference,
        },
        name: full.try_into().map_err(err)?,
        deref: false,
    })
    .map_err(err)?;
    Ok(())
}

/// The object a ref points to directly; symbolic refs are left to git.
fn direct_target(repo: &gix::Repository, full: &str) -> Result<ObjectId> {
    let r = repo.find_reference(full).map_err(err)?;
    match r.target() {
        TargetRef::Object(id) => Ok(id.to_owned()),
        TargetRef::Symbolic(_) => unsupported("symbolic ref"),
    }
}

impl GixBackend {
    /// Runs a ref edit on a blocking thread. On success it is reported to
    /// the command log with `argv` and the returned line; on failure the
    /// CLI retries and reports for itself.
    async fn write<F>(&self, argv: Vec<String>, f: F) -> Result<()>
    where
        F: FnOnce(&gix::Repository) -> Result<String> + Send + 'static,
    {
        let started = Instant::now();
        let line = self.blocking(f).await?;
        self.runner
            .report_in_process(Some(self.repo_id), argv, &[line], 0, started.elapsed());
        Ok(())
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    std::iter::once("gix")
        .chain(parts.iter().copied())
        .map(str::to_owned)
        .collect()
}

impl BranchOps for GixBackend {
    async fn check_branch_name(&self, _name: &str) -> Result<Option<String>> {
        unsupported("check-ref-format")
    }

    async fn create_branch(&self, name: &str, start: Option<&str>, checkout: bool) -> Result<()> {
        if checkout {
            return unsupported("create and switch");
        }
        plain_name(name)?;
        let mut cmd = argv(&["branch", name]);
        cmd.extend(start.map(str::to_owned));
        let (name, start) = (name.to_owned(), start.map(str::to_owned));
        self.write(cmd, move |repo| {
            no_ref_hooks(repo)?;
            match repo.config_snapshot().string("branch.autoSetupMerge") {
                None => {}
                Some(v) if v == "true" || v == "false" => {}
                Some(_) => return unsupported("branch.autoSetupMerge"),
            }
            // Like `git branch <name>`: without a start point it starts at
            // the current branch, named so in the reflog ("HEAD" if detached).
            let current = repo
                .head()
                .map_err(err)?
                .referent_name()
                .map(|n| text(n.shorten()));
            let rev = start
                .as_deref()
                .or(current.as_deref())
                .unwrap_or("HEAD")
                .to_owned();
            let rev = rev.as_str();
            if start.is_some() {
                let found = repo.try_find_reference(rev).ok().flatten();
                if found.is_some_and(|r| r.name().as_bstr().starts_with(b"refs/remotes/")) {
                    return unsupported("branch from a remote-tracking branch");
                }
            }
            let id = resolve(repo, rev)?;
            repo.reference(
                format!("refs/heads/{name}").as_str(),
                id,
                PreviousValue::MustNotExist,
                format!("branch: Created from {rev}"),
            )
            .map_err(err)?;
            Ok(format!("Created branch {name} at {}.", short(id)))
        })
        .await
    }

    async fn switch_branch(&self, _name: &str, _remote: bool) -> Result<()> {
        unsupported("switch")
    }

    async fn checkout_detached(&self, _rev: &str) -> Result<()> {
        unsupported("checkout")
    }

    async fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        plain_name(name)?;
        let cmd = argv(&["branch", if force { "-D" } else { "-d" }, name]);
        let name = name.to_owned();
        self.write(cmd, move |repo| {
            no_ref_hooks(repo)?;
            let config = repo.config_snapshot();
            if config
                .plumbing()
                .section("branch", Some(name.as_bytes().as_bstr()))
                .is_ok()
            {
                return unsupported("branch with config (upstream…)");
            }
            let full = format!("refs/heads/{name}");
            let tip = direct_target(repo, &full)?;
            if in_use(repo, &full)? {
                return unsupported("branch in use");
            }
            if !force {
                let Some(head) = head_id(repo)? else {
                    return unsupported("delete without HEAD");
                };
                let merged = tip == head
                    || repo
                        .merge_base(tip, head)
                        .is_ok_and(|base| base.is_some_and(|b| b.detach() == tip));
                if !merged {
                    return unsupported("branch not fully merged");
                }
            }
            delete_ref(repo, &full, tip)?;
            Ok(format!("Deleted branch {name} (was {}).", short(tip)))
        })
        .await
    }

    async fn delete_branches(&self, _names: &[String], _force: bool) -> Result<()> {
        unsupported("delete multiple branches")
    }

    async fn rename_branch(&self, _old: &str, _new: &str) -> Result<()> {
        unsupported("rename branch (reflog and config move with it)")
    }

    async fn delete_remote_branch(&self, _remote: &str, _branch: &str) -> Result<()> {
        unsupported("push")
    }

    async fn delete_remote_branches(&self, _remote: &str, _branches: &[String]) -> Result<()> {
        unsupported("push")
    }
}

impl TagOps for GixBackend {
    async fn create_tag(
        &self,
        name: &str,
        target: Option<&str>,
        message: Option<&str>,
    ) -> Result<()> {
        if message.is_some() {
            return unsupported("annotated tag");
        }
        plain_name(name)?;
        let mut cmd = argv(&["tag", name]);
        cmd.extend(target.map(str::to_owned));
        let (name, target) = (name.to_owned(), target.map(str::to_owned));
        self.write(cmd, move |repo| {
            no_ref_hooks(repo)?;
            // Like `git tag`: the object the revision names, not peeled.
            let rev = target.as_deref().unwrap_or("HEAD");
            let id = repo.rev_parse_single(rev).map_err(err)?.detach();
            repo.reference(
                format!("refs/tags/{name}").as_str(),
                id,
                PreviousValue::MustNotExist,
                "",
            )
            .map_err(err)?;
            Ok(format!("Created tag {name} at {}.", short(id)))
        })
        .await
    }

    async fn delete_tag(&self, name: &str) -> Result<()> {
        plain_name(name)?;
        let cmd = argv(&["tag", "-d", name]);
        let name = name.to_owned();
        self.write(cmd, move |repo| {
            no_ref_hooks(repo)?;
            let full = format!("refs/tags/{name}");
            let tip = direct_target(repo, &full)?;
            delete_ref(repo, &full, tip)?;
            Ok(format!("Deleted tag '{name}' (was {}).", short(tip)))
        })
        .await
    }

    async fn push_tag(&self, _remote: &str, _name: &str) -> Result<()> {
        unsupported("push")
    }
}
