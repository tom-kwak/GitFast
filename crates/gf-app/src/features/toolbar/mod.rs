//! Repository toolbar. Holds only the main git commands — fetch / pull /
//! push (with live counts and progress), stash — plus refresh at the far
//! right. View toggles belong to the sections they affect, not here; new
//! branches are made from the sidebar (BRANCHES header, a branch's menu).

use gf_git::Progress;
use iced::widget::{container, row, space};
use iced::{Element, Length, Padding};

use crate::repo_tab::RepoTab;
use crate::theme::{Tokens, style};
use crate::widgets::{self, colored, dim};

#[derive(Debug, Clone)]
pub enum Message {
    Fetch,
    Pull,
    Push,
    Stash,
    Refresh,
    Cancel,
}

/// A running network operation.
#[derive(Debug, Clone)]
pub struct Sync {
    pub label: String,
    pub progress: Option<Progress>,
}

/// Labels and tooltips for pull/push with the counts against the current
/// upstream: `⇣ pull (3)` = commits to receive, `⇡ push (2)` = commits to
/// send. Counts are as of the last fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCounts {
    pub pull_label: String,
    pub pull_hint: String,
    pub push_label: String,
    pub push_hint: String,
}

impl Default for SyncCounts {
    fn default() -> Self {
        Self {
            pull_label: "⇣ pull".into(),
            pull_hint: "fetch and integrate the upstream branch".into(),
            push_label: "⇡ push".into(),
            push_hint: "upload your commits to the upstream branch".into(),
        }
    }
}

/// `⇣ pull (3)`, or just `⇣ pull` when there is nothing to do.
fn count_label(base: &str, n: u32) -> String {
    if n == 0 {
        base.to_owned()
    } else {
        format!("{base} ({n})")
    }
}

pub fn sync_counts(head: &gf_core::model::HeadInfo, unpublished: Option<u32>) -> SyncCounts {
    match (&head.upstream, &head.branch) {
        (Some(up), _) => SyncCounts {
            pull_label: count_label("⇣ pull", head.behind),
            pull_hint: format!(
                "{} commit(s) on {up} you don't have yet — as of the last fetch (⟳ fetch to update)",
                head.behind
            ),
            push_label: count_label("⇡ push", head.ahead),
            push_hint: format!("{} local commit(s) not on {up} yet", head.ahead),
        },
        (None, Some(branch)) => {
            let n = unpublished.unwrap_or(0);
            SyncCounts {
                pull_label: "⇣ pull (—)".into(),
                pull_hint: format!("`{branch}` has no upstream yet — nothing to pull"),
                push_label: if n == 0 {
                    "⇡ push".into()
                } else {
                    format!("⇡ push ({n} new)")
                },
                push_hint: format!("publish `{branch}`: {n} commit(s) no remote has yet"),
            }
        }
        (None, None) => SyncCounts {
            pull_label: "⇣ pull (—)".into(),
            pull_hint: "HEAD is detached — check out a branch to pull".into(),
            push_label: "⇡ push (—)".into(),
            push_hint: "HEAD is detached — create a branch to push".into(),
        },
    }
}

/// `⟳ fetch (5m)`: how long ago the last fetch was — i.e. how fresh the
/// pull/push counts are. `(never)` before the first fetch.
pub fn fetch_label(
    last: Option<std::time::SystemTime>,
    remotes: usize,
    now: std::time::SystemTime,
) -> (String, String) {
    let from = match remotes {
        0 => "no remotes".to_owned(),
        1 => "1 remote".to_owned(),
        n => format!("{n} remotes"),
    };
    match last {
        None => (
            "⟳ fetch (never)".into(),
            format!(
                "never fetched — download new commits from {from} (does not change your files)"
            ),
        ),
        Some(at) => {
            let age = now.duration_since(at).unwrap_or_default();
            let short = crate::widgets::format::age(age);
            (
                format!("⟳ fetch ({short})"),
                format!(
                    "last fetched {} — download new commits from {from}; pull/push counts are as of the last fetch",
                    if short == "now" {
                        "just now".to_owned()
                    } else {
                        format!("{short} ago")
                    }
                ),
            )
        }
    }
}

pub fn view<'a>(
    t: &'a Tokens,
    tab: &'a RepoTab,
    sync: Option<&'a Sync>,
    spinner: &'a str,
) -> Element<'a, Message> {
    let has_remote = tab.snapshot.as_ref().is_some_and(|s| !s.remotes.is_empty());
    let busy = sync.is_some();
    let enabled = |m: Message| (!busy && has_remote).then_some(m);
    let counts = tab
        .snapshot
        .as_ref()
        .map(|s| sync_counts(&s.status.head, s.unpublished))
        .unwrap_or_default();
    let fetch = fetch_label(
        tab.snapshot.as_ref().and_then(|s| s.last_fetch),
        tab.snapshot.as_ref().map_or(0, |s| s.remotes.len()),
        std::time::SystemTime::now(),
    );

    let mut bar = row![
        widgets::tip(
            t,
            widgets::text_button(t, &fetch.0, None, enabled(Message::Fetch)),
            fetch.1,
        ),
        widgets::tip(
            t,
            widgets::text_button(t, &counts.pull_label, None, enabled(Message::Pull)),
            counts.pull_hint.clone(),
        ),
        widgets::tip(
            t,
            widgets::text_button(t, &counts.push_label, None, enabled(Message::Push)),
            counts.push_hint.clone(),
        ),
        dim(t, "│"),
        widgets::text_button(t, "≡ stash", None, Some(Message::Stash)),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);

    if !has_remote {
        bar = bar.push(dim(t, "no remote — add one with `remote add origin <url>`"));
    }
    bar = bar.push(space().width(Length::Fill));

    if let Some(s) = sync {
        let progress = match &s.progress {
            Some(p) => format!("{} {}% ({}/{})", p.phase, p.percent, p.current, p.total),
            None => "working…".into(),
        };
        bar = bar
            .push(colored(
                t,
                format!("{spinner} {} · {progress}", s.label),
                t.warning(),
            ))
            .push(widgets::text_button(
                t,
                "cancel",
                Some("Esc"),
                Some(Message::Cancel),
            ));
    }
    bar = bar.push(widgets::tip(
        t,
        widgets::text_button(t, "↻ refresh", Some("⌘R"), Some(Message::Refresh)),
        "reload status, branches and history".into(),
    ));

    container(bar)
        .padding(Padding::from([2, 4]))
        .width(Length::Fill)
        .style(style::toolbar(t))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gf_core::model::HeadInfo;

    fn head(branch: Option<&str>, upstream: Option<&str>, ahead: u32, behind: u32) -> HeadInfo {
        HeadInfo {
            oid: None,
            branch: branch.map(Into::into),
            upstream: upstream.map(Into::into),
            ahead,
            behind,
        }
    }

    #[test]
    fn labels_show_counts_against_upstream() {
        let c = sync_counts(&head(Some("main"), Some("origin/main"), 2, 3), None);
        assert_eq!(c.pull_label, "⇣ pull (3)");
        assert_eq!(c.push_label, "⇡ push (2)");
        assert!(c.pull_hint.contains("origin/main"));
        let c = sync_counts(&head(Some("main"), Some("origin/main"), 0, 0), None);
        assert_eq!(
            (c.pull_label.as_str(), c.push_label.as_str()),
            ("⇣ pull", "⇡ push"),
            "zero counts are left out"
        );
    }

    #[test]
    fn unpublished_and_detached() {
        let c = sync_counts(&head(Some("topic"), None, 0, 0), Some(5));
        assert_eq!(c.push_label, "⇡ push (5 new)");
        assert_eq!(c.pull_label, "⇣ pull (—)");
        let c = sync_counts(&head(None, None, 0, 0), None);
        assert_eq!(c.push_label, "⇡ push (—)");
    }

    #[test]
    fn fetch_label_shows_age() {
        use std::time::{Duration, SystemTime};
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(fetch_label(None, 1, now).0, "⟳ fetch (never)");
        let five_min = now - Duration::from_secs(300);
        let (label, hint) = fetch_label(Some(five_min), 2, now);
        assert_eq!(label, "⟳ fetch (5m)");
        assert!(
            hint.contains("5m ago") && hint.contains("2 remotes"),
            "{hint}"
        );
        assert_eq!(fetch_label(Some(now), 1, now).0, "⟳ fetch (now)");
    }
}
