//! Commit history list with ref decorations, and a `git show`-style detail
//! box for the selected commit. (The graph view replaces the list in M3.)

use std::collections::HashMap;
use std::sync::Arc;

use gf_core::model::{Commit, Oid, RefKind};
use iced::widget::{Column, column, container, row};
use iced::{Color, Element, Length, Padding};

use crate::feature::{Cx, Feature, Update};
use crate::repo_tab::Snapshot;
use crate::theme::{Tokens, style};
use crate::widgets::{self, colored, dim, format, mono};

#[derive(Debug, Default)]
pub struct State {
    /// Revision being shown; `None` = HEAD (the snapshot's commits).
    revision: Option<String>,
    /// Commits for `revision`, loaded by the app.
    commits: Option<Arc<Vec<Commit>>>,
    loading: bool,
    selected: Option<Oid>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(Oid),
}

impl State {
    /// Starts showing `revision`; the app loads its commits and calls
    /// [`Self::set_commits`].
    pub fn set_revision(&mut self, revision: Option<String>) {
        self.loading = revision.is_some();
        self.revision = revision;
        self.commits = None;
    }

    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    pub fn set_commits(&mut self, revision: &str, commits: Arc<Vec<Commit>>) {
        // Ignore results for a revision the user already navigated away from.
        if self.revision.as_deref() == Some(revision) {
            self.commits = Some(commits);
            self.loading = false;
        }
    }

    pub fn load_failed(&mut self) {
        self.loading = false;
        self.revision = None;
    }

    fn commits<'a>(&'a self, snap: &'a Snapshot) -> &'a [Commit] {
        match (&self.revision, &self.commits) {
            (None, _) => &snap.commits,
            (Some(_), Some(c)) => c,
            (Some(_), None) => &[],
        }
    }
}

/// `oid → [(label, color)]` for branch/tag badges.
fn decorations<'a>(t: &Tokens, snap: &'a Snapshot) -> HashMap<&'a Oid, Vec<(String, Color)>> {
    let mut map: HashMap<&Oid, Vec<(String, Color)>> = HashMap::new();
    for b in &snap.branches {
        let (label, color) = match b.name.kind() {
            RefKind::LocalBranch if b.is_head => (format!("HEAD → {}", b.name.short()), t.head()),
            RefKind::LocalBranch => (b.name.short().to_owned(), t.branch()),
            _ => (b.name.short().to_owned(), t.remote()),
        };
        map.entry(&b.target).or_default().push((label, color));
    }
    for tag in &snap.tags {
        map.entry(&tag.target)
            .or_default()
            .push((format!("◆ {}", tag.name.short()), t.tag()));
    }
    if snap.status.head.branch.is_none() {
        if let Some(oid) = &snap.status.head.oid {
            map.entry(oid)
                .or_default()
                .insert(0, ("HEAD".into(), t.head()));
        }
    }
    map
}

fn detail<'a>(t: &'a Tokens, c: &'a Commit) -> Element<'a, Message> {
    let mut col = column![
        colored(t, format!("commit {}", c.id), t.warning()),
        mono(t, format!("Author: {} <{}>", c.author.name, c.author.email)),
        mono(t, format!("Date:   {}", format::datetime(c.author.time))),
    ];
    if c.is_merge() {
        let parents: Vec<_> = c.parents.iter().map(Oid::short).collect();
        col = col.push(mono(t, format!("Merge:  {}", parents.join(" "))));
    }
    if c.committer.email != c.author.email {
        col = col.push(dim(
            t,
            format!("Committed by {} <{}>", c.committer.name, c.committer.email),
        ));
    }
    col = col.push(mono(t, format!("\n    {}", c.summary)));
    for line in c.body.lines() {
        col = col.push(mono(t, format!("    {line}")));
    }
    container(widgets::scroll(col.spacing(1)))
        .padding(Padding::from([4, 6]))
        .width(Length::Fill)
        .height(Length::Fixed(t.font_size * 12.0))
        .style(style::bar(t))
        .into()
}

impl Feature for State {
    type Message = Message;
    type Output = std::convert::Infallible;

    fn update(&mut self, message: Message) -> Update<Message, Self::Output> {
        match message {
            Message::Select(oid) => {
                // Clicking the selected commit again closes the detail box.
                self.selected = if self.selected.as_ref() == Some(&oid) {
                    None
                } else {
                    Some(oid)
                };
            }
        }
        Update::none()
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let title = match &self.revision {
            Some(r) => format!("HISTORY · {}", r.rsplit('/').next().unwrap_or(r)),
            None => "HISTORY · HEAD".into(),
        };
        let Some(snap) = cx.snapshot else {
            return widgets::panel(t, title, widgets::empty_hint(t, "loading…"));
        };
        if self.loading {
            return widgets::panel(t, title, widgets::empty_hint(t, "loading history…"));
        }
        let commits = self.commits(snap);
        if commits.is_empty() {
            return widgets::panel(
                t,
                title,
                widgets::empty_hint(
                    t,
                    "no commits yet — stage files and run `commit -m \"first\"`",
                ),
            );
        }

        let decor = decorations(t, snap);
        let list = commits.iter().fold(Column::new(), |col, c| {
            let mut line = row![colored(t, c.id.short(), t.warning())].spacing(8);
            if let Some(refs) = decor.get(&c.id) {
                for (label, color) in refs {
                    line = line.push(colored(t, format!("({label})"), *color));
                }
            }
            line = line
                .push(mono(t, format::truncate(&c.summary, 90)).width(Length::Fill))
                .push(dim(t, format::truncate(&c.author.name, 18)))
                .push(dim(t, format::datetime(c.author.time)));
            let selected = self.selected.as_ref() == Some(&c.id);
            col.push(widgets::list_row(
                t,
                line,
                selected,
                Message::Select(c.id.clone()),
            ))
        });

        let mut content = column![widgets::scroll(list)].spacing(4);
        if let Some(c) = self
            .selected
            .as_ref()
            .and_then(|s| commits.iter().find(|c| &c.id == s))
        {
            content = content.push(detail(t, c));
        }
        widgets::panel(t, format!("{title} ({})", commits.len()), content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_revision_results_are_ignored() {
        let mut s = State::default();
        s.set_revision(Some("refs/heads/a".into()));
        s.set_revision(Some("refs/heads/b".into()));
        s.set_commits("refs/heads/a", Arc::new(Vec::new()));
        assert!(s.loading, "result for a must not complete b");
        s.set_commits("refs/heads/b", Arc::new(Vec::new()));
        assert!(!s.loading);
    }
}
