//! Working tree status and per-file line counts through gix — the reads
//! done on every refresh. Must equal `git status --porcelain=v2` and
//! `git diff --numstat` (see `tests/gix_contract.rs`); anything this code
//! does not model (submodules, intent-to-add, external diff drivers…)
//! answers `Unsupported` so the git CLI is used.
//!
//! Full diffs (`DiffOps::diff`) stay on the CLI: hunk placement must match
//! git's xdiff heuristics exactly, and a diff is one process per click, not
//! a hot path.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use gf_core::Result;
use gf_core::model::{
    ChangeKind, ConflictKind, FileDiff, HeadInfo, RefName, StatusEntry, StatusReport,
};
use gix::ObjectId;
use gix::bstr::{BStr, BString, ByteSlice};
use gix::diff::blob::pipeline::{Mode, WorktreeRoots};
use gix::diff::blob::platform::prepare_diff::Operation;
use gix::diff::blob::platform::resource::Data;
use gix::diff::blob::{Diff, ResourceKind};
use gix::diff::index::Change as TreeIndexChange;
use gix::index::entry::Mode as EntryMode;
use gix::status::index_worktree::Item as WorktreeItem;
use gix::status::plumbing::index_as_worktree::{Change, Conflict, EntryStatus};

use super::gix_backend::{err, head_id, oid, text, unsupported, upstream};
use super::{DiffOps, GixBackend, StatusOps};
use crate::cli::parse::bytes_to_path;
use crate::cli::parse::numstat::LineCounts;
use crate::ops::diff::{DiffOptions, DiffTarget, FileVersion};

pub(super) fn path(p: &BStr) -> PathBuf {
    bytes_to_path(p.as_bytes())
}

/// Submodules are checked like `--ignore-submodules=none`: a new commit,
/// modified or untracked content all make one show as modified.
/// `check_dirty` stops at the first sign of change — enough for status,
/// not for numstat, which tells modified content from untracked files.
fn submodules(check_dirty: bool) -> gix::status::Submodule {
    gix::status::Submodule::Given {
        ignore: gix::submodule::config::Ignore::None,
        check_dirty,
    }
}

/// What `git diff --numstat` reports for a submodule: `1 1` when another
/// commit is checked out, `0 0` for modified content at the same commit,
/// nothing when it only has untracked files.
fn submodule_counts(status: &gix::submodule::Status) -> Option<LineCounts> {
    if status.checked_out_head_id.is_some() && status.checked_out_head_id != status.index_id {
        return Some(LineCounts {
            added: 1,
            removed: 1,
        });
    }
    let modified = status.changes.iter().flatten().any(|item| {
        !matches!(
            item,
            gix::status::Item::IndexWorktree(WorktreeItem::DirectoryContents { entry, .. })
                if entry.status == gix::dir::entry::Status::Untracked
        )
    });
    modified.then_some(LineCounts {
        added: 0,
        removed: 0,
    })
}

fn is_dirty(status: &gix::submodule::Status) -> bool {
    status.is_dirty() == Some(true)
}

/// `T` in porcelain: the kind of object changed (file ↔ symlink ↔ gitlink).
fn same_type(a: EntryMode, b: EntryMode) -> bool {
    let kind = |m: EntryMode| {
        if m.contains(EntryMode::SYMLINK) {
            1
        } else if m.contains(EntryMode::COMMIT) {
            2
        } else {
            0
        }
    };
    kind(a) == kind(b)
}

fn conflict(c: Conflict) -> ConflictKind {
    match c {
        Conflict::BothDeleted => ConflictKind::BothDeleted,
        Conflict::AddedByUs => ConflictKind::AddedByUs,
        Conflict::DeletedByThem => ConflictKind::DeletedByThem,
        Conflict::AddedByThem => ConflictKind::AddedByThem,
        Conflict::DeletedByUs => ConflictKind::DeletedByUs,
        Conflict::BothAdded => ConflictKind::BothAdded,
        Conflict::BothModified => ConflictKind::BothModified,
    }
}

#[derive(Default)]
struct Sides {
    index: Option<ChangeKind>,
    worktree: Option<ChangeKind>,
    orig: Option<BString>,
    conflict: Option<ConflictKind>,
}

fn head_info(repo: &gix::Repository) -> Result<HeadInfo> {
    let head = repo.head().map_err(err)?;
    let tip = head_id(repo)?;
    let mut info = HeadInfo {
        oid: tip.map(oid),
        ..HeadInfo::default()
    };
    let Some(full) = head.referent_name() else {
        return Ok(info); // detached
    };
    info.branch = Some(text(full.shorten()));
    let Some(tip) = tip else {
        return unsupported("upstream of an unborn branch");
    };
    if let Some(up) = upstream(repo, full, tip)? {
        info.upstream = Some(RefName::new(up.name.as_str()).short().to_owned());
        info.ahead = up.ahead;
        info.behind = up.behind;
    }
    Ok(info)
}

fn status(repo: &gix::Repository) -> Result<StatusReport> {
    status_with(repo, submodules(true))
}

/// [`status`] with configurable submodule checks. Both callers report
/// individual untracked files, like `git status --untracked-files=all`.
pub(super) fn status_with(
    repo: &gix::Repository,
    submodule_check: gix::status::Submodule,
) -> Result<StatusReport> {
    let head = head_info(repo)?;

    let mut tracked: BTreeMap<BString, Sides> = BTreeMap::new();
    let mut untracked: Vec<BString> = Vec::new();
    let iter = repo
        .status(gix::progress::Discard)
        .map_err(err)?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .index_worktree_rewrites(None)
        .index_worktree_submodules(submodule_check)
        .into_iter(Vec::<BString>::new())
        .map_err(err)?;
    for item in iter {
        match item.map_err(err)? {
            gix::status::Item::TreeIndex(change) => {
                let (location, kind, orig) = match change {
                    TreeIndexChange::Addition { location, .. } => {
                        (location, ChangeKind::Added, None)
                    }
                    TreeIndexChange::Deletion { location, .. } => {
                        (location, ChangeKind::Deleted, None)
                    }
                    TreeIndexChange::Modification {
                        location,
                        previous_entry_mode,
                        entry_mode,
                        ..
                    } => {
                        let kind = if same_type(previous_entry_mode, entry_mode) {
                            ChangeKind::Modified
                        } else {
                            ChangeKind::TypeChanged
                        };
                        (location, kind, None)
                    }
                    TreeIndexChange::Rewrite {
                        source_location,
                        location,
                        copy,
                        ..
                    } => {
                        let kind = if copy {
                            ChangeKind::Copied
                        } else {
                            ChangeKind::Renamed
                        };
                        (location, kind, Some(source_location.into_owned()))
                    }
                };
                let sides = tracked.entry(location.into_owned()).or_default();
                sides.index = Some(kind);
                sides.orig = orig;
            }
            gix::status::Item::IndexWorktree(item) => match item {
                WorktreeItem::Modification {
                    rela_path, status, ..
                } => {
                    let sides = tracked.entry(rela_path).or_default();
                    match status {
                        EntryStatus::Conflict { summary, .. } => {
                            sides.conflict = Some(conflict(summary));
                        }
                        EntryStatus::Change(Change::Removed) => {
                            sides.worktree = Some(ChangeKind::Deleted);
                        }
                        EntryStatus::Change(Change::Type { .. }) => {
                            sides.worktree = Some(ChangeKind::TypeChanged);
                        }
                        EntryStatus::Change(Change::Modification { .. }) => {
                            sides.worktree = Some(ChangeKind::Modified);
                        }
                        EntryStatus::Change(Change::SubmoduleModification(sub)) => {
                            if is_dirty(&sub) {
                                sides.worktree = Some(ChangeKind::Modified);
                            }
                        }
                        EntryStatus::IntentToAdd => {
                            return unsupported("intent-to-add entry");
                        }
                        // Only the cached stat is outdated.
                        EntryStatus::NeedsUpdate(_) => {}
                    }
                }
                WorktreeItem::DirectoryContents { entry, .. } => {
                    if entry.status == gix::dir::entry::Status::Untracked {
                        untracked.push(entry.rela_path);
                    }
                }
                WorktreeItem::Rewrite { .. } => return unsupported("worktree rename"),
            },
        }
    }

    let mut entries = Vec::new();
    for (p, sides) in tracked {
        if let Some(kind) = sides.conflict {
            entries.push(StatusEntry::Conflicted {
                path: path(p.as_ref()),
                kind,
            });
        } else if sides.index.is_some() || sides.worktree.is_some() {
            entries.push(StatusEntry::Changed {
                path: path(p.as_ref()),
                orig_path: sides.orig.as_ref().map(|o| path(o.as_ref())),
                index: sides.index.unwrap_or(ChangeKind::Unmodified),
                worktree: sides.worktree.unwrap_or(ChangeKind::Unmodified),
            });
        }
    }
    untracked.sort();
    entries.extend(untracked.into_iter().map(|p| StatusEntry::Untracked {
        path: path(p.as_ref()),
    }));
    Ok(StatusReport { head, entries })
}

/// One side of a line count: blob id (null = from the worktree or
/// missing), its mode and path.
type Side<'a> = Option<(ObjectId, EntryMode, &'a BStr)>;

fn kind(mode: EntryMode) -> Result<gix::object::tree::EntryKind> {
    mode.to_tree_entry_mode()
        .map(|m| m.kind())
        .ok_or_else(|| err("unknown entry mode"))
}

/// Lines added/removed between `old` and `new` like `git diff --numstat`;
/// `None` for binary files.
fn line_counts(
    repo: &gix::Repository,
    cache: &mut gix::diff::blob::Platform,
    old: Side<'_>,
    new: Side<'_>,
) -> Result<Option<LineCounts>> {
    let (_, any_mode, any_path) = old.or(new).ok_or_else(|| err("nothing to compare"))?;
    // A gitlink diffs as one `Subproject commit <id>` line per side.
    let gitlink = |side: Side<'_>| side.is_some_and(|(_, m, _)| m == EntryMode::COMMIT);
    if gitlink(old) || gitlink(new) {
        if old.is_some_and(|o| !gitlink(Some(o))) || new.is_some_and(|n| !gitlink(Some(n))) {
            return unsupported("type change to or from a submodule");
        }
        let changed = old.map(|(id, ..)| id) != new.map(|(id, ..)| id);
        return Ok(Some(LineCounts {
            added: u32::from(new.is_some() && changed),
            removed: u32::from(old.is_some() && changed),
        }));
    }
    let null = ObjectId::null(repo.object_hash());
    for (side, which) in [
        (old, ResourceKind::OldOrSource),
        (new, ResourceKind::NewOrDestination),
    ] {
        let (id, mode, p) = side.unwrap_or((null, any_mode, any_path));
        cache
            .set_resource(id, kind(mode)?, p, which, &repo.objects)
            .map_err(err)?;
    }
    let outcome = cache.prepare_diff().map_err(err)?;
    let counts = match outcome.operation {
        Operation::SourceOrDestinationIsBinary => None,
        Operation::ExternalCommand { .. } => return unsupported("external diff driver"),
        Operation::InternalDiff { algorithm } => {
            let input = outcome.interned_input();
            let diff = Diff::compute(algorithm, &input);
            let (mut added, mut removed) = (diff.count_additions(), diff.count_removals());
            // Lines are compared without their newline, but git counts a
            // last line that only gained or lost its newline as changed.
            if let (Data::Buffer { buf: a, .. }, Data::Buffer { buf: b, .. }) =
                (outcome.old.data, outcome.new.data)
            {
                let flipped =
                    !a.is_empty() && !b.is_empty() && a.ends_with(b"\n") != b.ends_with(b"\n");
                if flipped {
                    let (la, lb) = (input.before.len(), input.after.len());
                    let last_kept = u32::try_from(la - 1)
                        .ok()
                        .zip(u32::try_from(lb - 1).ok())
                        .is_some_and(|(i, j)| !diff.is_removed(i) && !diff.is_added(j));
                    if !last_kept {
                        return unsupported("changed last line without newline");
                    }
                    added += 1;
                    removed += 1;
                }
            }
            Some(LineCounts { added, removed })
        }
    };
    cache.clear_resource_cache_keep_allocation();
    Ok(counts)
}

fn numstat(repo: &gix::Repository, staged: bool) -> Result<HashMap<PathBuf, Option<LineCounts>>> {
    let mut out = HashMap::new();
    if staged {
        let index = repo.index_or_empty().map_err(err)?;
        let tree = repo.head_tree_id_or_empty().map_err(err)?;
        let mut changes = Vec::new();
        repo.tree_index_status(
            &tree,
            &index,
            None,
            gix::status::tree_index::TrackRenames::Given(gix::diff::Rewrites::default()),
            |change, _, _| {
                changes.push(change.into_owned());
                Ok(std::ops::ControlFlow::Continue(()))
            },
        )
        .map_err(err)?;
        let mut cache = repo.diff_resource_cache_for_tree_diff().map_err(err)?;
        for change in changes {
            let (key, counts) = match &change {
                TreeIndexChange::Addition {
                    location,
                    entry_mode,
                    id,
                    ..
                } => (
                    location,
                    line_counts(
                        repo,
                        &mut cache,
                        None,
                        Some((id.clone().into_owned(), *entry_mode, location.as_ref())),
                    )?,
                ),
                TreeIndexChange::Deletion {
                    location,
                    entry_mode,
                    id,
                    ..
                } => (
                    location,
                    line_counts(
                        repo,
                        &mut cache,
                        Some((id.clone().into_owned(), *entry_mode, location.as_ref())),
                        None,
                    )?,
                ),
                TreeIndexChange::Modification {
                    location,
                    previous_entry_mode,
                    previous_id,
                    entry_mode,
                    id,
                    ..
                } => (
                    location,
                    line_counts(
                        repo,
                        &mut cache,
                        Some((
                            previous_id.clone().into_owned(),
                            *previous_entry_mode,
                            location.as_ref(),
                        )),
                        Some((id.clone().into_owned(), *entry_mode, location.as_ref())),
                    )?,
                ),
                TreeIndexChange::Rewrite {
                    source_location,
                    source_entry_mode,
                    source_id,
                    location,
                    entry_mode,
                    id,
                    ..
                } => (
                    location,
                    line_counts(
                        repo,
                        &mut cache,
                        Some((
                            source_id.clone().into_owned(),
                            *source_entry_mode,
                            source_location.as_ref(),
                        )),
                        Some((id.clone().into_owned(), *entry_mode, location.as_ref())),
                    )?,
                ),
            };
            out.insert(path(key.as_ref()), counts);
        }
    } else {
        let workdir = repo
            .workdir()
            .ok_or_else(|| err("bare repository"))?
            .to_path_buf();
        let mut cache = repo
            .diff_resource_cache(
                Mode::ToGit,
                WorktreeRoots {
                    old_root: None,
                    new_root: Some(workdir),
                },
            )
            .map_err(err)?;
        let iter = repo
            .status(gix::progress::Discard)
            .map_err(err)?
            .untracked_files(gix::status::UntrackedFiles::None)
            .index_worktree_rewrites(None)
            .index_worktree_submodules(submodules(false))
            .into_index_worktree_iter(Vec::<BString>::new())
            .map_err(err)?;
        let null = ObjectId::null(repo.object_hash());
        for item in iter {
            let WorktreeItem::Modification {
                entry,
                rela_path,
                status,
                ..
            } = item.map_err(err)?
            else {
                continue;
            };
            let p: &BStr = rela_path.as_ref();
            let old = Some((entry.id, entry.mode, p));
            let counts = match status {
                EntryStatus::Change(Change::Modification { .. } | Change::Type { .. }) => {
                    line_counts(repo, &mut cache, old, Some((null, entry.mode, p)))?
                }
                EntryStatus::Change(Change::Removed) => line_counts(repo, &mut cache, old, None)?,
                EntryStatus::NeedsUpdate(_) => continue,
                EntryStatus::Conflict { .. } => return unsupported("numstat of a conflict"),
                EntryStatus::IntentToAdd => return unsupported("intent-to-add entry"),
                EntryStatus::Change(Change::SubmoduleModification(sub)) => {
                    match submodule_counts(&sub) {
                        Some(counts) => Some(counts),
                        None => continue,
                    }
                }
            };
            out.insert(path(p), counts);
        }
    }
    Ok(out)
}

impl StatusOps for GixBackend {
    async fn status(&self) -> Result<StatusReport> {
        self.blocking(status).await
    }
}

impl DiffOps for GixBackend {
    async fn file_bytes(&self, _path: &Path, _version: &FileVersion) -> Result<Vec<u8>> {
        unsupported("full file contents")
    }

    async fn diff(&self, _target: &DiffTarget, _opts: &DiffOptions) -> Result<Vec<FileDiff>> {
        unsupported("diff hunks")
    }

    async fn numstat(&self, staged: bool) -> Result<HashMap<PathBuf, Option<LineCounts>>> {
        self.blocking(move |repo| numstat(repo, staged)).await
    }
}
