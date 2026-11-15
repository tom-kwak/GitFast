//! Sidebar: local branches, remote branches, tags and stashes as a
//! collapsible tree with a quick filter. Selecting an item shows its
//! history (or diff, for stashes) and an action bar for it; section headers
//! have `+` buttons to create new items.

use std::collections::HashSet;

use gf_core::model::{Branch, RefKind, StashEntry, Tag};
use iced::widget::text::Wrapping;
use iced::widget::{Row, button, column, row, space, text_input};
use iced::{Element, Length, Padding};

use crate::feature::{Cx, Feature, Update};
use crate::repo_tab::Snapshot;
use crate::theme::{Tokens, style};
use crate::widgets::virtual_list::{ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim, format};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Branches,
    Remotes,
    Tags,
    Stashes,
}

/// What is selected in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Full ref name of a branch or tag.
    Ref(String),
    Stash(usize),
}

#[derive(Debug, Default)]
pub struct State {
    collapsed: HashSet<Section>,
    filter: String,
    selected: Option<Selection>,
    scroll: ScrollPos,
}

/// One row of the flattened tree.
#[derive(Debug, Clone, Copy)]
enum SideRow<'a> {
    Header(Section, &'static str, usize, Option<(Action, &'static str)>),
    Branch(&'a Branch),
    Tag(&'a Tag),
    Stash(&'a StashEntry),
    Hint(&'static str),
}

#[derive(Debug, Clone)]
pub enum Message {
    Toggle(Section),
    Filter(String),
    Select(Selection),
    /// Action bar / header buttons.
    Act(Action),
    Scrolled(ScrollPos),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NewBranch,
    NewTag,
    NewStash,
    Checkout,
    Rename,
    Delete,
    Push,
    StashApply,
    StashPop,
}

/// Requests to the app. Destructive ones are confirmed by the app.
#[derive(Debug, Clone)]
pub enum Output {
    /// Show history of this ref; `None` = back to the default history.
    ShowRevision(Option<String>),
    ShowStash(StashEntry),
    NewBranch {
        from: Option<String>,
    },
    NewTag,
    NewStash,
    Checkout(Branch),
    RenameBranch(String),
    DeleteBranch(Branch),
    PushBranch(Branch),
    DeleteTag(String),
    PushTag(String),
    StashApply(usize),
    StashPop(usize),
    StashDrop(usize),
}

impl State {
    /// Forgets a selection that no longer exists (deleted branch, dropped
    /// stash). Returns false when it was forgotten.
    pub fn retain_existing(&mut self, snapshot: &Snapshot) -> bool {
        let exists = match &self.selected {
            None => return true,
            Some(Selection::Ref(r)) => {
                snapshot.branches.iter().any(|b| b.name.as_str() == r)
                    || snapshot.tags.iter().any(|t| t.name.as_str() == r)
            }
            Some(Selection::Stash(i)) => *i < snapshot.stashes.len(),
        };
        if !exists {
            self.selected = None;
        }
        exists
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    fn matches(&self, name: &str) -> bool {
        self.filter.is_empty() || name.to_lowercase().contains(&self.filter.to_lowercase())
    }

    fn is_selected_ref(&self, full: &str) -> bool {
        matches!(&self.selected, Some(Selection::Ref(r)) if r == full)
    }

    fn header<'a>(
        &self,
        t: &'a Tokens,
        section: Section,
        label: &str,
        count: usize,
        add: Option<(Action, &'static str)>,
    ) -> Element<'a, Message> {
        let arrow = if self.collapsed.contains(&section) {
            "▸"
        } else {
            "▾"
        };
        let toggle = button(row![dim(t, arrow), dim(t, format!("{label} ({count})"))].spacing(6))
            .padding(Padding::from([1, 6]))
            .style(style::row_button(t, false))
            .on_press(Message::Toggle(section));
        match add {
            Some((action, hint)) => row![
                toggle,
                widgets::icon_button(t, "+", t.accent(), hint, Message::Act(action))
            ]
            .align_y(iced::Alignment::Center)
            .into(),
            None => toggle.into(),
        }
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
            colored(t, b.name.short(), name_color).wrapping(Wrapping::None)
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
            self.is_selected_ref(full),
            Message::Select(Selection::Ref(full.to_owned())),
        );
        let hint = match &b.upstream {
            Some(up) if up.gone => format!(
                "{} — upstream {} was deleted on the remote",
                b.name.short(),
                up.name.short()
            ),
            Some(up) => format!(
                "{} → {}  (↑{} ahead, ↓{} behind)",
                b.name.short(),
                up.name.short(),
                up.ahead,
                up.behind
            ),
            None if b.name.kind() == RefKind::LocalBranch => {
                format!("{} · not published yet", b.name.short())
            }
            None => format!("{} · {}", b.name.short(), b.target.short()),
        };
        widgets::tip(t, row, hint)
    }

    fn tag_row<'a>(&self, t: &'a Tokens, tag: &'a Tag) -> Element<'a, Message> {
        let full = tag.name.as_str();
        let line = row![
            colored(t, "◆", t.tag()),
            colored(t, tag.name.short(), t.tag()).wrapping(Wrapping::None)
        ]
        .spacing(6);
        let row = widgets::list_row(
            t,
            line,
            self.is_selected_ref(full),
            Message::Select(Selection::Ref(full.to_owned())),
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

    fn stash_row<'a>(&self, t: &'a Tokens, s: &'a StashEntry) -> Element<'a, Message> {
        let line = row![
            colored(t, "≡", t.accent()),
            colored(t, s.message.clone(), t.fg).wrapping(Wrapping::None)
        ]
        .spacing(6);
        let selected = self.selected == Some(Selection::Stash(s.index));
        widgets::tip(
            t,
            widgets::list_row(
                t,
                line,
                selected,
                Message::Select(Selection::Stash(s.index)),
            ),
            format!("{} · {}", s.refname(), format::datetime(s.time)),
        )
    }

    /// Action bar for the current selection.
    fn actions<'a>(&self, t: &'a Tokens, snap: &Snapshot) -> Option<Element<'a, Message>> {
        let btn =
            |label: &str, a: Action| widgets::text_button(t, label, None, Some(Message::Act(a)));
        let danger =
            |label: &str, a: Action| widgets::danger_button(t, label, Some(Message::Act(a)));
        let bar: Row<'a, Message> = match self.selected.as_ref()? {
            Selection::Ref(r) => match snap.branches.iter().find(|b| b.name.as_str() == r) {
                Some(b) if b.name.kind() == RefKind::LocalBranch && b.is_head => row![
                    btn("branch from here", Action::NewBranch),
                    btn("rename", Action::Rename),
                    btn("push", Action::Push),
                ],
                Some(b) if b.name.kind() == RefKind::LocalBranch => row![
                    btn("checkout", Action::Checkout),
                    btn("rename", Action::Rename),
                    btn("push", Action::Push),
                    danger("delete", Action::Delete),
                ],
                Some(_) => row![
                    btn("checkout", Action::Checkout),
                    btn("branch from here", Action::NewBranch),
                    danger("delete", Action::Delete),
                ],
                None => row![btn("push", Action::Push), danger("delete", Action::Delete)],
            },
            Selection::Stash(_) => row![
                btn("apply", Action::StashApply),
                btn("pop", Action::StashPop),
                danger("drop", Action::Delete),
            ],
        };
        Some(bar.spacing(2).wrap().into())
    }

    fn output_for(&self, action: Action, snap: Option<&Snapshot>) -> Option<Output> {
        match action {
            Action::NewBranch => {
                let from = match &self.selected {
                    Some(Selection::Ref(r)) => Some(r.clone()),
                    _ => None,
                };
                return Some(Output::NewBranch { from });
            }
            Action::NewTag => return Some(Output::NewTag),
            Action::NewStash => return Some(Output::NewStash),
            _ => {}
        }
        let snap = snap?;
        match self.selected.as_ref()? {
            Selection::Stash(i) => match action {
                Action::StashApply => Some(Output::StashApply(*i)),
                Action::StashPop => Some(Output::StashPop(*i)),
                Action::Delete => Some(Output::StashDrop(*i)),
                _ => None,
            },
            Selection::Ref(r) => {
                if let Some(b) = snap.branches.iter().find(|b| b.name.as_str() == r) {
                    match action {
                        Action::Checkout => Some(Output::Checkout(b.clone())),
                        Action::Rename => Some(Output::RenameBranch(b.name.short().to_owned())),
                        Action::Delete => Some(Output::DeleteBranch(b.clone())),
                        Action::Push => Some(Output::PushBranch(b.clone())),
                        _ => None,
                    }
                } else {
                    let tag = r.strip_prefix("refs/tags/")?.to_owned();
                    match action {
                        Action::Delete => Some(Output::DeleteTag(tag)),
                        Action::Push => Some(Output::PushTag(tag)),
                        _ => None,
                    }
                }
            }
        }
    }

    /// Like [`Feature::update`] but with access to the snapshot, which the
    /// action bar needs to resolve the selection into a branch/tag/stash.
    pub fn update_with(
        &mut self,
        message: Message,
        snap: Option<&Snapshot>,
    ) -> Update<Message, Output> {
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
            Message::Select(sel) => {
                if self.selected.as_ref() == Some(&sel) {
                    self.selected = None;
                    return Update::output(Output::ShowRevision(None));
                }
                self.selected = Some(sel.clone());
                match sel {
                    Selection::Ref(r) => Update::output(Output::ShowRevision(Some(r))),
                    Selection::Stash(i) => snap
                        .and_then(|s| s.stashes.get(i))
                        .map_or_else(Update::none, |s| {
                            Update::output(Output::ShowStash(s.clone()))
                        }),
                }
            }
            Message::Act(action) => self
                .output_for(action, snap)
                .map_or_else(Update::none, Update::output),
            Message::Scrolled(pos) => {
                self.scroll = pos;
                Update::none()
            }
        }
    }
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        self.update_with(message, None)
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
        let stashes: Vec<_> = snap
            .stashes
            .iter()
            .filter(|s| self.matches(&s.message))
            .collect();
        let mut rows: Vec<SideRow<'a>> = Vec::new();
        let open = |sec: Section| !self.collapsed.contains(&sec);

        rows.push(SideRow::Header(
            Section::Branches,
            "BRANCHES",
            locals.len(),
            Some((Action::NewBranch, "new branch…")),
        ));
        if open(Section::Branches) {
            if locals.is_empty() {
                rows.push(SideRow::Hint(if self.filter.is_empty() {
                    "none yet — make a first commit"
                } else {
                    "no match"
                }));
            }
            rows.extend(locals.iter().map(|b| SideRow::Branch(b)));
        }
        rows.push(SideRow::Header(
            Section::Remotes,
            "REMOTES",
            remotes.len(),
            None,
        ));
        if open(Section::Remotes) {
            if remotes.is_empty() {
                rows.push(SideRow::Hint(if snap.remotes.is_empty() {
                    "none — `remote add <name> <url>`"
                } else {
                    "none fetched — try fetch"
                }));
            }
            rows.extend(remotes.iter().map(|b| SideRow::Branch(b)));
        }
        rows.push(SideRow::Header(
            Section::Tags,
            "TAGS",
            tags.len(),
            Some((Action::NewTag, "new tag on HEAD…")),
        ));
        if open(Section::Tags) {
            if tags.is_empty() {
                rows.push(SideRow::Hint("none — + creates one"));
            }
            rows.extend(tags.iter().map(|tg| SideRow::Tag(tg)));
        }
        rows.push(SideRow::Header(
            Section::Stashes,
            "STASHES",
            stashes.len(),
            Some((Action::NewStash, "stash changes…")),
        ));
        if open(Section::Stashes) {
            if stashes.is_empty() {
                rows.push(SideRow::Hint("none — + puts changes aside"));
            }
            rows.extend(stashes.iter().map(|s| SideRow::Stash(s)));
        }

        let name_cols = rows
            .iter()
            .map(|r| match r {
                SideRow::Branch(b) => columns(b.name.short()) + 12,
                SideRow::Tag(tg) => columns(tg.name.short()),
                SideRow::Stash(s) => columns(&s.message),
                SideRow::Hint(h) => columns(h),
                SideRow::Header(..) => 18,
            })
            .max()
            .unwrap_or(0);
        let content_width = t.cols(4 + name_cols) + 16.0;
        let list = virtual_list(
            "gf-refs",
            rows.len(),
            (t.font_size * 1.8).round(),
            content_width,
            self.scroll,
            move |i| match rows[i] {
                SideRow::Header(sec, label, count, add) => self.header(t, sec, label, count, add),
                SideRow::Branch(b) => self.branch_row(t, b),
                SideRow::Tag(tg) => self.tag_row(t, tg),
                SideRow::Stash(s) => self.stash_row(t, s),
                SideRow::Hint(h) => widgets::empty_hint(t, h),
            },
            Message::Scrolled,
        );

        let footer: Element<'a, Message> = self
            .actions(t, snap)
            .unwrap_or_else(|| row![dim(t, "select an item for actions"), space()].into());
        widgets::panel(
            t,
            "REFS".into(),
            column![filter, list, footer].spacing(4).width(Length::Fill),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gf_core::model::RefName;

    fn branch(name: &str, head: bool) -> Branch {
        Branch {
            name: RefName::local_branch(name),
            target: "a".repeat(40).parse().expect("oid"),
            upstream: None,
            is_head: head,
        }
    }

    fn snap() -> Snapshot {
        Snapshot {
            branches: vec![branch("main", true), branch("dev", false)],
            ..Snapshot::default()
        }
    }

    #[test]
    fn select_toggles_and_reports() {
        let mut s = State::default();
        let out = s
            .update(Message::Select(Selection::Ref("refs/heads/dev".into())))
            .output;
        assert!(matches!(out, Some(Output::ShowRevision(Some(ref r))) if r == "refs/heads/dev"));
        let out = s
            .update(Message::Select(Selection::Ref("refs/heads/dev".into())))
            .output;
        assert!(matches!(out, Some(Output::ShowRevision(None))));
    }

    #[test]
    fn actions_resolve_against_snapshot() {
        let snap = snap();
        let mut s = State::default();
        let _ = s.update_with(
            Message::Select(Selection::Ref("refs/heads/dev".into())),
            Some(&snap),
        );
        let out = s
            .update_with(Message::Act(Action::Delete), Some(&snap))
            .output;
        assert!(matches!(out, Some(Output::DeleteBranch(b)) if b.name.short() == "dev"));
        let out = s
            .update_with(Message::Act(Action::NewBranch), Some(&snap))
            .output;
        assert!(matches!(out, Some(Output::NewBranch { from: Some(f) }) if f == "refs/heads/dev"));
    }

    #[test]
    fn filter_is_case_insensitive() {
        let mut s = State::default();
        let _ = s.update(Message::Filter("FEAT".into()));
        assert!(s.matches("feature/login"));
        assert!(!s.matches("main"));
    }

    #[test]
    fn removed_selection_is_forgotten() {
        let mut s = State::default();
        let _ = s.update(Message::Select(Selection::Stash(3)));
        assert!(!s.retain_existing(&snap()));
        assert!(s.selected.is_none());
    }
}
