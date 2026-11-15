//! Commit history with a terminal-style graph (`● │ ├─╯`), ref badges and a
//! detail box with actions for the selected commit. Selecting a commit shows
//! its changed files and diff in the commit changes view.
//!
//! The list is virtualized (only visible rows are built) and the graph
//! column has a maximum width, so repositories with many merged branches
//! still show the commit text.

use std::collections::HashMap;
use std::sync::Arc;

use gf_core::model::{Commit, Oid, RefKind};
use gf_git::graph::GraphRow;
use iced::widget::text::Wrapping;
use iced::widget::{Row, column, container, row};
use iced::{Color, Element, Length, Padding};

use crate::feature::{Cx, Feature, Update};
use crate::repo_tab::{History, Snapshot};
use crate::theme::{Tokens, style};
use crate::widgets::graph_canvas;
use crate::widgets::virtual_list::{ScrollPos, columns, virtual_list};
use crate::widgets::{self, colored, dim, format, mono};

/// Scrollable id of the commit list.
pub const LIST_ID: &str = "gf-history-list";
/// Ref badges shown per commit before collapsing into `+N`.
const MAX_BADGES: usize = 2;

#[derive(Debug, Default)]
pub struct State {
    /// Revision being shown; `None` = the snapshot's history.
    revision: Option<String>,
    /// History for `revision`, loaded by the app.
    data: Option<Arc<History>>,
    loading: bool,
    selected: Option<Oid>,
    scroll: ScrollPos,
    /// Author/date/message box of the selected commit; closed by default so
    /// the list keeps its space. The action buttons are always shown.
    show_details: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(Oid),
    Scrolled(ScrollPos),
    ToggleAllBranches,
    BranchHere,
    TagHere,
    Checkout,
    CopyId,
    BackToHead,
    ToggleDetails,
}

#[derive(Debug, Clone)]
pub enum Output {
    ShowCommit(Oid),
    ToggleAllBranches,
    NewBranchAt(Oid),
    NewTagAt(Oid),
    CheckoutDetached(Oid),
    CopyId(Oid),
    BackToHead,
}

impl State {
    /// Starts showing `revision`; the app loads it and calls [`Self::loaded`].
    pub fn set_revision(&mut self, revision: Option<String>) {
        self.loading = revision.is_some();
        self.revision = revision;
        self.data = None;
        self.scroll = ScrollPos::default();
    }

    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    pub fn loaded(&mut self, revision: &str, data: Arc<History>) {
        // Ignore results for a revision the user already navigated away from.
        if self.revision.as_deref() == Some(revision) {
            self.data = Some(data);
            self.loading = false;
        }
    }

    pub fn load_failed(&mut self) {
        self.loading = false;
        self.revision = None;
    }

    fn history<'a>(&'a self, snap: &'a Snapshot) -> Option<&'a History> {
        match (&self.revision, &self.data) {
            (None, _) => Some(&snap.history),
            (Some(_), Some(d)) => Some(d),
            (Some(_), None) => None,
        }
    }
}

/// Row height for the list: one line of text plus padding.
fn row_height(t: &Tokens) -> f32 {
    (t.font_size * 1.7).round()
}

/// `oid → [(label, color)]` for branch/tag badges, HEAD first.
fn decorations<'a>(t: &Tokens, snap: &'a Snapshot) -> HashMap<&'a Oid, Vec<(String, Color)>> {
    let mut map: HashMap<&Oid, Vec<(String, Color)>> = HashMap::new();
    for b in &snap.branches {
        let (label, color) = match b.name.kind() {
            RefKind::LocalBranch if b.is_head => (format!("HEAD → {}", b.name.short()), t.head()),
            RefKind::LocalBranch => (b.name.short().to_owned(), t.branch()),
            _ => (b.name.short().to_owned(), t.remote()),
        };
        let list = map.entry(&b.target).or_default();
        if b.is_head {
            list.insert(0, (label, color));
        } else {
            list.push((label, color));
        }
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

/// Columns reserved for the author and date.
const AUTHOR_COLS: usize = 16;
const DATE_COLS: usize = 16;
/// Badges + summary are clipped to at most this many columns so author and
/// date stay aligned; the full text is in the detail box.
const MAX_SUMMARY_COLS: usize = 110;

/// Badges and summary as one text line, plus its width in columns.
fn summary_line(
    c: &Commit,
    badges: Option<&Vec<(String, Color)>>,
) -> (Vec<(String, Option<Color>)>, usize) {
    let mut parts: Vec<(String, Option<Color>)> = Vec::new();
    if let Some(refs) = badges {
        for (label, color) in refs.iter().take(MAX_BADGES) {
            parts.push((format!("({}) ", format::truncate(label, 28)), Some(*color)));
        }
        if refs.len() > MAX_BADGES {
            parts.push((format!("+{} ", refs.len() - MAX_BADGES), None));
        }
    }
    parts.push((c.summary.clone(), None));
    let cols = parts.iter().map(|(s, _)| columns(s)).sum();
    (parts, cols)
}

#[allow(clippy::too_many_arguments)]
fn commit_row<'a>(
    t: &'a Tokens,
    c: &'a Commit,
    g: Option<&'a GraphRow>,
    graph_px: f32,
    summary_px: f32,
    badges: Option<&Vec<(String, Color)>>,
    selected: bool,
) -> Element<'a, Message> {
    let mut line = Row::new()
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .height(Length::Fill);
    if let Some(g) = g {
        let colors = [0, 1, 2, 3, 4, 5].map(|i| t.lane(i));
        line = line.push(graph_canvas::row_graph(g, colors, t.bg, graph_px));
    }
    line = line.push(colored(t, c.id.short(), t.warning()).wrapping(Wrapping::None));

    let (parts, _) = summary_line(c, badges);
    let mut summary = Row::new();
    for (text, color) in parts {
        summary = summary.push(match color {
            Some(color) => colored(t, text, color).wrapping(Wrapping::None),
            None => mono(t, text).wrapping(Wrapping::None),
        });
    }
    line = line
        .push(
            container(summary)
                .width(Length::Fixed(summary_px))
                .clip(true),
        )
        .push(
            container(
                dim(t, format::truncate(&c.author.name, AUTHOR_COLS)).wrapping(Wrapping::None),
            )
            .width(Length::Fixed(t.cols(AUTHOR_COLS))),
        )
        .push(dim(t, format::datetime(c.author.time)).wrapping(Wrapping::None));
    let row = widgets::list_row_flush(t, line, selected, Message::Select(c.id.clone()));
    match badges {
        Some(refs) if refs.len() > MAX_BADGES => {
            let all: Vec<&str> = refs.iter().map(|(l, _)| l.as_str()).collect();
            widgets::tip(t, row, all.join(", "))
        }
        _ => row,
    }
}

/// Bar under the list for the selected commit: id, actions and a details
/// toggle; with details open, also author, date and the full message.
fn detail<'a>(t: &'a Tokens, c: &'a Commit, open: bool) -> Element<'a, Message> {
    let bar = row![
        colored(t, c.id.short(), t.warning()),
        mono(t, c.summary.clone())
            .wrapping(Wrapping::None)
            .width(Length::Fill),
        widgets::text_button(t, "branch here", None, Some(Message::BranchHere)),
        widgets::text_button(t, "tag here", None, Some(Message::TagHere)),
        widgets::text_button(t, "checkout", None, Some(Message::Checkout)),
        widgets::text_button(t, "copy id", None, Some(Message::CopyId)),
        widgets::tip(
            t,
            widgets::text_button(
                t,
                if open { "▾ details" } else { "▸ details" },
                None,
                Some(Message::ToggleDetails),
            ),
            if open {
                "hide author, date and message"
            } else {
                "show author, date and full message"
            }
            .into(),
        ),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);

    if !open {
        return container(bar)
            .padding(Padding::from([2, 6]))
            .width(Length::Fill)
            .clip(true)
            .style(style::bar(t))
            .into();
    }

    let mut col = column![
        colored(t, format!("commit {}", c.id), t.warning()).wrapping(Wrapping::None),
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
    container(column![bar, widgets::scroll(col.spacing(1))].spacing(4))
        .padding(Padding::from([4, 6]))
        .width(Length::Fill)
        .height(Length::Fixed(t.font_size * 10.0))
        .style(style::bar(t))
        .into()
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        let selected = self.selected.clone();
        match message {
            Message::Select(oid) => {
                self.selected = Some(oid.clone());
                Update::output(Output::ShowCommit(oid))
            }
            Message::Scrolled(pos) => {
                self.scroll = pos;
                Update::none()
            }
            Message::ToggleAllBranches => Update::output(Output::ToggleAllBranches),
            Message::BranchHere => {
                selected.map_or_else(Update::none, |o| Update::output(Output::NewBranchAt(o)))
            }
            Message::TagHere => {
                selected.map_or_else(Update::none, |o| Update::output(Output::NewTagAt(o)))
            }
            Message::Checkout => selected.map_or_else(Update::none, |o| {
                Update::output(Output::CheckoutDetached(o))
            }),
            Message::CopyId => {
                selected.map_or_else(Update::none, |o| Update::output(Output::CopyId(o)))
            }
            Message::BackToHead => Update::output(Output::BackToHead),
            Message::ToggleDetails => {
                self.show_details = !self.show_details;
                Update::none()
            }
        }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        let all = cx.all_branches;
        let scope = match &self.revision {
            Some(r) => r.rsplit('/').next().unwrap_or(r).to_owned(),
            None if all => "all branches".into(),
            None => "HEAD".into(),
        };
        let mut toolbar = row![dim(t, format!("showing: {scope}")).width(Length::Fill)]
            .spacing(4)
            .align_y(iced::Alignment::Center);
        if self.revision.is_some() {
            toolbar = toolbar.push(widgets::text_button(
                t,
                "back to all",
                None,
                Some(Message::BackToHead),
            ));
        } else {
            toolbar = toolbar.push(widgets::text_button(
                t,
                if all {
                    "■ all branches"
                } else {
                    "□ all branches"
                },
                None,
                Some(Message::ToggleAllBranches),
            ));
        }

        let Some(snap) = cx.snapshot else {
            return widgets::panel(t, "HISTORY".into(), widgets::empty_hint(t, "loading…"));
        };
        if self.loading {
            return widgets::panel(
                t,
                "HISTORY".into(),
                column![toolbar, widgets::empty_hint(t, "loading history…")],
            );
        }
        let Some(history) = self.history(snap) else {
            return widgets::panel(t, "HISTORY".into(), toolbar);
        };
        if history.commits.is_empty() {
            return widgets::panel(
                t,
                "HISTORY".into(),
                column![
                    toolbar,
                    widgets::empty_hint(
                        t,
                        "no commits yet — open the working tree (⌘E) to stage and commit"
                    )
                ],
            );
        }

        let decor = decorations(t, snap);
        let lanes = history.graph.iter().map(|g| g.width).max().unwrap_or(1);
        let graph_px = graph_canvas::width_for(lanes);
        let summary_cols = history
            .commits
            .iter()
            .map(|c| summary_line(c, decor.get(&c.id)).1)
            .max()
            .unwrap_or(0)
            .clamp(20, MAX_SUMMARY_COLS);
        let summary_px = t.cols(summary_cols);
        // graph, hash, summary, author, date + gaps and padding.
        let content_width =
            graph_px + t.cols(7 + AUTHOR_COLS + DATE_COLS) + summary_px + 4.0 * 6.0 + 12.0;
        let selected = self.selected.as_ref();
        let list = virtual_list(
            LIST_ID,
            history.commits.len(),
            row_height(t),
            content_width,
            self.scroll,
            move |i| {
                let c = &history.commits[i];
                commit_row(
                    t,
                    c,
                    history.graph.get(i),
                    graph_px,
                    summary_px,
                    decor.get(&c.id),
                    selected == Some(&c.id),
                )
            },
            Message::Scrolled,
        );

        let mut content = column![toolbar, list].spacing(4);
        if let Some(c) = self
            .selected
            .as_ref()
            .and_then(|s| history.commits.iter().find(|c| &c.id == s))
        {
            content = content.push(detail(t, c, self.show_details));
        }
        widgets::panel(t, format!("HISTORY ({})", history.commits.len()), content)
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
        s.loaded("refs/heads/a", Arc::default());
        assert!(s.loading, "result for a must not complete b");
        s.loaded("refs/heads/b", Arc::default());
        assert!(!s.loading);
    }

    #[test]
    fn actions_need_a_selection() {
        let mut s = State::default();
        assert!(s.update(Message::BranchHere).output.is_none());
        let oid: Oid = "a".repeat(40).parse().expect("oid");
        let _ = s.update(Message::Select(oid.clone()));
        assert!(matches!(s.update(Message::CopyId).output, Some(Output::CopyId(o)) if o == oid));
    }

    #[test]
    fn changing_revision_resets_scroll() {
        let mut s = State::default();
        let _ = s.update(Message::Scrolled(ScrollPos {
            offset: 500.0,
            height: 300.0,
        }));
        s.set_revision(Some("refs/heads/x".into()));
        assert_eq!(s.scroll, ScrollPos::default());
    }

    #[test]
    fn details_start_closed_and_toggle() {
        let mut s = State::default();
        assert!(!s.show_details);
        let _ = s.update(Message::ToggleDetails);
        assert!(s.show_details);
        // Stays open when another commit is selected.
        let _ = s.update(Message::Select("c".repeat(40).parse().expect("oid")));
        assert!(s.show_details);
    }
}
