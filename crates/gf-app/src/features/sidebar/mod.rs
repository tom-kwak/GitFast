//! Sidebar: local branches, remote branches and tags as a collapsible tree
//! with a quick filter. Clicking a ref shows its history; clicking it again
//! returns to HEAD.

use std::collections::HashSet;

use gf_core::model::{Branch, RefKind, Tag};
use iced::widget::{Column, column, row, text_input};
use iced::{Element, Length};

use crate::feature::{Cx, Feature, Update};
use crate::repo_tab::Snapshot;
use crate::theme::{Tokens, style};
use crate::widgets::{self, colored, dim, mono};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Branches,
    Remotes,
    Tags,
}

#[derive(Debug, Default)]
pub struct State {
    collapsed: HashSet<Section>,
    filter: String,
    /// Full ref name whose history is shown (`None` = HEAD).
    selected: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Toggle(Section),
    Filter(String),
    Select(String),
}

#[derive(Debug, Clone)]
pub enum Output {
    /// Show history of this revision; `None` = back to HEAD.
    ShowRevision(Option<String>),
}

impl State {
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Forgets a selection whose ref no longer exists (deleted branch).
    pub fn retain_existing(&mut self, snapshot: &Snapshot) {
        if let Some(sel) = &self.selected {
            let exists = snapshot.branches.iter().any(|b| b.name.as_str() == sel)
                || snapshot.tags.iter().any(|t| t.name.as_str() == sel);
            if !exists {
                self.selected = None;
            }
        }
    }

    fn matches(&self, name: &str) -> bool {
        self.filter.is_empty() || name.to_lowercase().contains(&self.filter.to_lowercase())
    }

    fn header<'a>(
        &self,
        t: &'a Tokens,
        section: Section,
        label: &str,
        count: usize,
    ) -> Element<'a, Message> {
        let arrow = if self.collapsed.contains(&section) {
            "▸"
        } else {
            "▾"
        };
        widgets::list_row(
            t,
            row![dim(t, arrow), dim(t, format!("{label} ({count})"))].spacing(6),
            false,
            Message::Toggle(section),
        )
    }

    fn branch_row<'a>(&self, t: &'a Tokens, b: &'a Branch) -> Element<'a, Message> {
        let full = b.name.as_str();
        let (marker, marker_color) = if b.is_head {
            ("●", t.head())
        } else {
            ("○", t.dim)
        };
        let name_color = if b.name.kind() == RefKind::RemoteBranch {
            t.remote()
        } else {
            t.branch()
        };
        let mut line = row![
            colored(t, marker, marker_color),
            colored(t, b.name.short(), name_color)
        ]
        .spacing(6);
        if let Some(up) = &b.upstream {
            if up.gone {
                line = line.push(colored(t, "⚠ gone", t.warning()));
            } else {
                if up.ahead > 0 {
                    line = line.push(colored(t, format!("↑{}", up.ahead), t.success()));
                }
                if up.behind > 0 {
                    line = line.push(colored(t, format!("↓{}", up.behind), t.warning()));
                }
            }
        }
        let row = widgets::list_row(
            t,
            line,
            self.selected.as_deref() == Some(full),
            Message::Select(full.to_owned()),
        );
        let hint = match &b.upstream {
            Some(up) if up.gone => format!(
                "{} — upstream {} was deleted on the remote",
                b.name.short(),
                up.name.short()
            ),
            Some(up) => format!(
                "{} → {}  (↑{} ahead, ↓{} behind) · click: show history",
                b.name.short(),
                up.name.short(),
                up.ahead,
                up.behind
            ),
            None => format!(
                "{} · {} · click: show history",
                b.name.short(),
                b.target.short()
            ),
        };
        widgets::tip(t, row, hint)
    }

    fn tag_row<'a>(&self, t: &'a Tokens, tag: &'a Tag) -> Element<'a, Message> {
        let full = tag.name.as_str();
        let line = row![
            colored(t, "◆", t.tag()),
            colored(t, tag.name.short(), t.tag())
        ]
        .spacing(6);
        let row = widgets::list_row(
            t,
            line,
            self.selected.as_deref() == Some(full),
            Message::Select(full.to_owned()),
        );
        let hint = match &tag.message {
            Some(m) => format!(
                "{} → {} · {}",
                tag.name.short(),
                tag.target.short(),
                m.lines().next().unwrap_or("")
            ),
            None => format!(
                "{} → {} (lightweight)",
                tag.name.short(),
                tag.target.short()
            ),
        };
        widgets::tip(t, row, hint)
    }
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message {
            Message::Toggle(section) => {
                if !self.collapsed.remove(&section) {
                    self.collapsed.insert(section);
                }
                Update::none()
            }
            Message::Filter(f) => {
                self.filter = f;
                Update::none()
            }
            Message::Select(name) => {
                self.selected = if self.selected.as_deref() == Some(&name) {
                    None
                } else {
                    Some(name)
                };
                Update::output(Output::ShowRevision(self.selected.clone()))
            }
        }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let filter = text_input("filter refs…", &self.filter)
            .on_input(Message::Filter)
            .size(t.small())
            .padding(3)
            .style(style::field_input(t));

        let Some(snap) = cx.snapshot else {
            return widgets::panel(
                t,
                "REFS".into(),
                column![filter, widgets::empty_hint(t, "loading…")],
            );
        };

        let locals: Vec<_> = snap
            .branches
            .iter()
            .filter(|b| b.name.kind() == RefKind::LocalBranch && self.matches(b.name.short()))
            .collect();
        let remotes: Vec<_> = snap
            .branches
            .iter()
            .filter(|b| b.name.kind() == RefKind::RemoteBranch && self.matches(b.name.short()))
            .collect();
        let tags: Vec<_> = snap
            .tags
            .iter()
            .filter(|tg| self.matches(tg.name.short()))
            .collect();

        let mut list = Column::new().spacing(1);

        list = list.push(self.header(t, Section::Branches, "BRANCHES", locals.len()));
        if !self.collapsed.contains(&Section::Branches) {
            if locals.is_empty() {
                list = list.push(widgets::empty_hint(
                    t,
                    if self.filter.is_empty() {
                        "no branches yet — make a first commit"
                    } else {
                        "no match"
                    },
                ));
            }
            for b in locals {
                list = list.push(self.branch_row(t, b));
            }
        }

        list = list.push(self.header(t, Section::Remotes, "REMOTES", remotes.len()));
        if !self.collapsed.contains(&Section::Remotes) {
            if remotes.is_empty() {
                list = list.push(widgets::empty_hint(
                    t,
                    if snap.remotes.is_empty() {
                        "none — `remote add <name> <url>`"
                    } else {
                        "none fetched — try `fetch`"
                    },
                ));
            }
            for b in remotes {
                list = list.push(self.branch_row(t, b));
            }
        }

        list = list.push(self.header(t, Section::Tags, "TAGS", tags.len()));
        if !self.collapsed.contains(&Section::Tags) {
            if tags.is_empty() {
                list = list.push(widgets::empty_hint(t, "none — `tag v1.0` creates one"));
            }
            for tg in tags {
                list = list.push(self.tag_row(t, tg));
            }
        }

        let showing = match &self.selected {
            Some(sel) => row![
                dim(t, "history:"),
                mono(t, sel.rsplit('/').next().unwrap_or(sel))
            ]
            .spacing(4),
            None => row![dim(t, "history: HEAD")],
        };

        widgets::panel(
            t,
            "REFS".into(),
            column![filter, widgets::scroll(list), showing]
                .spacing(4)
                .width(Length::Fill),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_toggles_and_reports() {
        let mut s = State::default();
        let out = s.update(Message::Select("refs/heads/dev".into())).output;
        assert!(matches!(out, Some(Output::ShowRevision(Some(ref r))) if r == "refs/heads/dev"));
        let out = s.update(Message::Select("refs/heads/dev".into())).output;
        assert!(matches!(out, Some(Output::ShowRevision(None))));
    }

    #[test]
    fn filter_is_case_insensitive() {
        let mut s = State::default();
        let _ = s.update(Message::Filter("FEAT".into()));
        assert!(s.matches("feature/login"));
        assert!(!s.matches("main"));
    }

    #[test]
    fn collapse_toggles() {
        let mut s = State::default();
        let _ = s.update(Message::Toggle(Section::Tags));
        assert!(s.collapsed.contains(&Section::Tags));
        let _ = s.update(Message::Toggle(Section::Tags));
        assert!(!s.collapsed.contains(&Section::Tags));
    }
}
