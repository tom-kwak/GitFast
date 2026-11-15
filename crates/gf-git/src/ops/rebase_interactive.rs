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
use crate::Repository;

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

impl Repository {
    /// Commits a rebase onto `base` would replay, oldest first. `None`
    /// base means from the root commit.
    pub async fn rebase_plan(&self, base: Option<&Oid>) -> Result<Vec<TodoItem>> {
        let revisions = match base {
            Some(b) => vec![format!("{b}..HEAD")],
            None => vec!["HEAD".into()],
        };
        let mut commits = self
            .log_cli(&LogQuery {
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

    /// Runs the interactive rebase described by `items` onto `base`
    /// (`None` = rewrite from the root).
    pub async fn rebase_interactive(
        &self,
        base: Option<&Oid>,
        items: &[TodoItem],
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

        let mut cmd = self.command(["rebase", "--interactive"]).env(
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
