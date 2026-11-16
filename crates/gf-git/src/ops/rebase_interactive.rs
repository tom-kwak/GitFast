//! Interactive rebase driven by a plan built in the UI (reorder, squash,
//! fixup, reword, drop) instead of an editor.
//!
//! The plan is turned into a todo list that git reads through
//! `GIT_SEQUENCE_EDITOR`. New messages (reword, squash) are applied with
//! `exec git commit --amend -F <file>` lines, so no editor is ever opened.
//! Message files live in `<git dir>/gf-rebase/` so they survive a stop on
//! conflicts until the rebase is continued.

use std::fmt::Write as _;

use gf_core::model::{Commit, Oid};
use gf_core::{GfError, Result};

use super::integrate::Integration;
use super::log::{LogOrder, LogQuery};
use crate::backend::{CliBackend, LogOps, RebaseOps, RefOps};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoAction {
    Pick,
    /// Keep the commit with a new message.
    Reword(String),
    /// Meld into the previous commit; the result gets this message.
    Squash(String),
    /// Meld into the previous commit, keeping the previous message.
    Fixup,
    Drop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub action: TodoAction,
    pub commit: Commit,
}

/// Whether the plan starts by melding into nothing: the first commit that
/// is kept (dropped ones do not count) is a squash or fixup.
pub fn melds_into_nothing(items: &[TodoItem]) -> bool {
    matches!(
        items
            .iter()
            .map(|i| &i.action)
            .find(|a| **a != TodoAction::Drop),
        Some(TodoAction::Squash(_) | TodoAction::Fixup)
    )
}

/// Where the commits `ids` sit in `items` (oldest first): a run of
/// adjacent rows, as `first..=last`. Errors name what is wrong.
pub fn squash_range(items: &[TodoItem], ids: &[Oid]) -> Result<std::ops::RangeInclusive<usize>> {
    let mut pos = ids
        .iter()
        .map(|id| {
            items
                .iter()
                .position(|i| &i.commit.id == id)
                .ok_or_else(|| {
                    GfError::Other(format!(
                        "{} is not on the current branch — check out the branch that has these commits",
                        id.short()
                    ))
                })
        })
        .collect::<Result<Vec<_>>>()?;
    pos.sort_unstable();
    pos.dedup();
    if pos.len() < 2 {
        return Err(GfError::Other(
            "select at least two commits to squash".into(),
        ));
    }
    if pos.windows(2).any(|w| w[1] != w[0] + 1) {
        return Err(GfError::Other(
            "only adjacent commits can be squashed; select a continuous range".into(),
        ));
    }
    Ok(pos[0]..=pos[pos.len() - 1])
}

/// Melds the rows in `range` into one commit with `message`: the oldest is
/// picked, the ones in between are fixups and the newest carries the new
/// message. Rows outside the range are left alone.
pub fn squash_plan(
    mut items: Vec<TodoItem>,
    range: std::ops::RangeInclusive<usize>,
    message: String,
) -> Vec<TodoItem> {
    let (first, last) = (*range.start(), *range.end());
    for item in &mut items[first + 1..last] {
        item.action = TodoAction::Fixup;
    }
    items[last].action = TodoAction::Squash(message);
    items
}

/// What the squash dialog shows before anything is rewritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SquashPreview {
    /// Parent of the oldest commit; `None` when the range starts at the
    /// root commit.
    pub base: Option<Oid>,
    /// The commits to meld, oldest first.
    pub commits: Vec<Commit>,
    /// How many of them are already on a remote (rewriting them needs a
    /// force push).
    pub pushed: usize,
}

/// Single-quotes a path for the shell git runs the sequence editor in.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Builds the todo text. `msg_file(i)` returns the path of the message
/// file for item `i` (only called for reword/squash).
pub fn todo_text(items: &[TodoItem], msg_file: impl Fn(usize) -> String) -> Result<String> {
    if melds_into_nothing(items) {
        return Err(GfError::Other("the first commit cannot be squashed or fixed up: there is nothing before it to meld into".into()));
    }
    let mut out = String::new();
    for (i, item) in items.iter().enumerate() {
        let id = &item.commit.id;
        let summary = item.commit.summary.replace('\n', " ");
        let amend = |out: &mut String| {
            let _ = writeln!(
                out,
                "exec git commit --amend --allow-empty --no-verify -F {}",
                sh_quote(&msg_file(i))
            );
        };
        match &item.action {
            TodoAction::Pick => {
                let _ = writeln!(out, "pick {id} {summary}");
            }
            TodoAction::Drop => {
                let _ = writeln!(out, "drop {id} {summary}");
            }
            TodoAction::Fixup => {
                let _ = writeln!(out, "fixup {id} {summary}");
            }
            TodoAction::Reword(_) => {
                let _ = writeln!(out, "pick {id} {summary}");
                amend(&mut out);
            }
            TodoAction::Squash(_) => {
                let _ = writeln!(out, "fixup {id} {summary}");
                amend(&mut out);
            }
        }
    }
    Ok(out)
}

/// Commits a rebase onto `base` would replay, oldest first (`None` = from
/// the root): the first-parent line `base..HEAD`. Shared by the backends,
/// which differ only in how they walk the log.
pub(crate) async fn plan(b: &impl LogOps, base: Option<&Oid>) -> Result<Vec<TodoItem>> {
    let revisions = match base {
        Some(b) => vec![format!("{b}..HEAD")],
        None => vec!["HEAD".into()],
    };
    let mut commits = b
        .log(&LogQuery {
            revisions,
            order: LogOrder::Topo,
            first_parent: true,
            ..LogQuery::default()
        })
        .await?;
    commits.reverse();
    Ok(commits
        .into_iter()
        .map(|commit| TodoItem {
            action: TodoAction::Pick,
            commit,
        })
        .collect())
}

/// Parent of the oldest of `ids` on the current branch's first-parent
/// line; `None` when that oldest commit is the root.
pub(crate) async fn squash_base(b: &impl LogOps, ids: &[Oid]) -> Result<Option<Oid>> {
    // Among the ids, the oldest is the one the others all descend from:
    // the last one in HEAD's first-parent order.
    let line = b
        .log(&LogQuery {
            revisions: vec!["HEAD".into()],
            order: LogOrder::Topo,
            first_parent: true,
            ..LogQuery::default()
        })
        .await?;
    let oldest = line
        .iter()
        .rev()
        .find(|c| ids.contains(&c.id))
        .ok_or_else(|| {
            GfError::Other(
                "these commits are not on the current branch — check out the branch that has them"
                    .into(),
            )
        })?;
    Ok(oldest.parents.first().cloned())
}

/// What the squash dialog shows for melding `ids`.
pub(crate) async fn preview(b: &(impl LogOps + RefOps), ids: &[Oid]) -> Result<SquashPreview> {
    let base = squash_base(b, ids).await?;
    let items = plan(b, base.as_ref()).await?;
    let range = squash_range(&items, ids)?;
    let commits: Vec<Commit> = items[range].iter().map(|i| i.commit.clone()).collect();
    let unpushed = b.unpushed(base.as_ref()).await?;
    let pushed = commits.iter().filter(|c| !unpushed.contains(&c.id)).count();
    Ok(SquashPreview {
        base,
        commits,
        pushed,
    })
}

impl RebaseOps for CliBackend {
    async fn rebase_plan(&self, base: Option<&Oid>) -> Result<Vec<TodoItem>> {
        plan(self, base).await
    }

    async fn squash_preview(&self, ids: &[Oid]) -> Result<SquashPreview> {
        preview(self, ids).await
    }

    async fn squash(&self, ids: &[Oid], message: &str, autostash: bool) -> Result<Integration> {
        let base = squash_base(self, ids).await?;
        let items = plan(self, base.as_ref()).await?;
        let range = squash_range(&items, ids)?;
        let items = squash_plan(items, range, message.to_owned());
        self.rebase_with(base.as_ref(), &items, autostash).await
    }

    async fn rebase_interactive(
        &self,
        base: Option<&Oid>,
        items: &[TodoItem],
    ) -> Result<Integration> {
        self.rebase_with(base, items, false).await
    }
}

impl CliBackend {
    async fn rebase_with(
        &self,
        base: Option<&Oid>,
        items: &[TodoItem],
        autostash: bool,
    ) -> Result<Integration> {
        let dir = self.handle().git_dir.join("gf-rebase");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await?;
        let msg_path = |i: usize| dir.join(format!("msg-{i}.txt"));
        for (i, item) in items.iter().enumerate() {
            if let TodoAction::Reword(m) | TodoAction::Squash(m) = &item.action {
                if m.trim().is_empty() {
                    return Err(GfError::Other(format!(
                        "commit {} needs a message",
                        item.commit.id.short()
                    )));
                }
                tokio::fs::write(msg_path(i), m).await?;
            }
        }
        let todo = todo_text(items, |i| msg_path(i).display().to_string())?;
        let todo_path = dir.join("todo");
        tokio::fs::write(&todo_path, todo).await?;

        let mut cmd = self.command(["rebase", "--interactive"]);
        if autostash {
            cmd = cmd.arg("--autostash");
        }
        let mut cmd = cmd.env(
            "GIT_SEQUENCE_EDITOR",
            format!("cp {}", sh_quote(&todo_path.display().to_string())),
        );
        cmd = match base {
            Some(b) => cmd.args(["--end-of-options", b.as_str()]),
            None => cmd.arg("--root"),
        };
        self.integrate_public(cmd).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gf_core::model::{Signature, Timestamp};

    fn commit(n: u8, summary: &str) -> Commit {
        let sig = Signature {
            name: String::new(),
            email: String::new(),
            time: Timestamp {
                seconds: 0,
                offset_minutes: 0,
            },
        };
        Commit {
            id: format!("{n:040x}").parse().expect("oid"),
            parents: vec![],
            author: sig.clone(),
            committer: sig,
            summary: summary.into(),
            body: String::new(),
        }
    }

    #[test]
    fn builds_todo_with_exec_for_messages() {
        let items = vec![
            TodoItem {
                action: TodoAction::Pick,
                commit: commit(1, "one"),
            },
            TodoItem {
                action: TodoAction::Reword("better".into()),
                commit: commit(2, "two"),
            },
            TodoItem {
                action: TodoAction::Squash("combined".into()),
                commit: commit(3, "three"),
            },
            TodoItem {
                action: TodoAction::Fixup,
                commit: commit(4, "four"),
            },
            TodoItem {
                action: TodoAction::Drop,
                commit: commit(5, "five"),
            },
        ];
        let todo = todo_text(&items, |i| format!("/tmp/it's {i}")).expect("todo");
        let lines: Vec<&str> = todo.lines().collect();
        assert!(lines[0].starts_with("pick 0000000000000000000000000000000000000001 one"));
        assert!(lines[1].starts_with("pick "));
        assert_eq!(
            lines[2],
            "exec git commit --amend --allow-empty --no-verify -F '/tmp/it'\\''s 1'"
        );
        assert!(lines[3].starts_with("fixup ") && lines[4].starts_with("exec "));
        assert!(lines[5].starts_with("fixup ") && lines[6].starts_with("drop "));
    }

    #[test]
    fn first_item_cannot_meld() {
        let items = vec![TodoItem {
            action: TodoAction::Fixup,
            commit: commit(1, "one"),
        }];
        assert!(todo_text(&items, |_| String::new()).is_err());
    }

    #[test]
    fn first_kept_item_cannot_meld_after_dropped_ones() {
        let items = vec![
            TodoItem {
                action: TodoAction::Drop,
                commit: commit(1, "one"),
            },
            TodoItem {
                action: TodoAction::Fixup,
                commit: commit(2, "two"),
            },
        ];
        assert!(todo_text(&items, |_| String::new()).is_err());
    }
}
