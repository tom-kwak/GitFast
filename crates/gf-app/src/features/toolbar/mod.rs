//! Repository toolbar. Holds only the main git commands — fetch / pull /
//! push (with live counts and progress), stash — plus refresh at the far
//! right. View toggles belong to the sections they affect, not here; new
//! branches are made from the sidebar (BRANCHES header, a branch's menu).

use gf_git::Progress;
use iced::widget::{container, row, space};
use iced::{Element, Length, Padding};

use crate::repo_tab::{RepoTab, Snapshot};
use crate::theme::{Tokens, style};
use crate::widgets::{self, PUSH_ARROW, colored, dim};

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
            push_label: format!("{PUSH_ARROW} push"),
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

/// `tags`: local tags the push remote does not have (they go with a push).
pub fn sync_counts(
    head: &gf_core::model::HeadInfo,
    unpublished: Option<u32>,
    tags: usize,
) -> SyncCounts {
    let with_tags = |commits: Option<String>| {
        let tags = match tags {
            0 => None,
            1 => Some("1 tag".to_owned()),
            n => Some(format!("{n} tags")),
        };
        match (commits, tags) {
            (None, None) => format!("{PUSH_ARROW} push"),
            (Some(c), None) => format!("{PUSH_ARROW} push ({c})"),
            (None, Some(t)) => format!("{PUSH_ARROW} push ({t})"),
            (Some(c), Some(t)) => format!("{PUSH_ARROW} push ({c} +{t})"),
        }
    };
    let tag_hint = if tags == 0 {
        String::new()
    } else {
        format!(" · {tags} tag(s) the remote does not have")
    };
    match (&head.upstream, &head.branch) {
        (Some(up), _) => SyncCounts {
            pull_label: count_label("⇣ pull", head.behind),
            pull_hint: format!(
                "{} commit(s) on {up} you don't have yet — as of the last fetch (⟳ fetch to update)",
                head.behind
            ),
            push_label: with_tags((head.ahead > 0).then(|| head.ahead.to_string())),
            push_hint: format!("{} local commit(s) not on {up} yet{tag_hint}", head.ahead),
        },
        (None, Some(branch)) => {
            let n = unpublished.unwrap_or(0);
            SyncCounts {
                pull_label: "⇣ pull (—)".into(),
                pull_hint: format!("`{branch}` has no upstream yet — nothing to pull"),
                push_label: with_tags((n > 0).then(|| format!("{n} new"))),
                push_hint: format!("publish `{branch}`: {n} commit(s) no remote has yet{tag_hint}"),
            }
        }
        (None, None) => SyncCounts {
            pull_label: "⇣ pull (—)".into(),
            pull_hint: "HEAD is detached — check out a branch to pull".into(),
            push_label: format!("{PUSH_ARROW} push (—)"),
            push_hint: "HEAD is detached — create a branch to push".into(),
        },
    }
}

/// Why the push count may leave tags out: gf could not list the tags of
/// these remotes yet (unreachable, or never fetched), so it cannot tell
/// which local tags are unpublished. Empty when every remote is listed.
pub fn unknown_tags_hint(unlisted: &[String]) -> String {
    match unlisted {
        [] => String::new(),
        [one] => format!(" · tags not counted: could not list `{one}`'s tags yet (⟳ fetch)"),
        many => format!(
            " · tags not counted: could not list the tags of {} remotes yet (⟳ fetch)",
            many.len()
        ),
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

fn sync_available(snapshot: &Snapshot) -> (bool, bool) {
    let head = &snapshot.status.head;
    if head.branch.is_none() || head.oid.is_none() {
        return (false, false);
    }
    let tracks = head.upstream.is_some();
    let pull = tracks && (head.behind > 0 || snapshot.last_fetch.is_none());
    let push_tags = snapshot
        .pending_tags
        .as_ref()
        .map_or_else(|| !snapshot.tags.is_empty(), |tags| !tags.names.is_empty());
    let push = !tracks || head.ahead > 0 || push_tags;
    (pull, push)
}

pub fn view<'a>(
    t: &'a Tokens,
    tab: &'a RepoTab,
    sync: Option<&'a Sync>,
    spinner: &'a str,
) -> Element<'a, Message> {
    let has_remote = tab.snapshot.as_ref().is_some_and(|s| !s.remotes.is_empty());
    let stash_hint = match tab.snapshot.as_ref() {
        Some(s) if s.has_changes => None,
        Some(_) => Some("nothing to stash"),
        None => Some("loading status…"),
    };
    let busy = sync.is_some();
    let enabled = |m: Message| (!busy && has_remote).then_some(m);
    let (pull, push) = tab
        .snapshot
        .as_deref()
        .map_or((false, false), sync_available);
    let mut counts = tab
        .snapshot
        .as_ref()
        .map(|s| {
            let tags = s.pending_tags.as_ref().map_or(0, |p| p.names.len());
            let mut c = sync_counts(&s.status.head, s.unpublished, tags);
            c.push_hint
                .push_str(&unknown_tags_hint(&s.unlisted_remotes));
            c
        })
        .unwrap_or_default();
    if !pull
        && tab
            .snapshot
            .as_ref()
            .is_some_and(|s| s.status.head.upstream.is_some())
    {
        counts.pull_hint = "no incoming commits at the last fetch · Fetch to check again".into();
    }
    if !push
        && tab
            .snapshot
            .as_ref()
            .is_some_and(|s| s.status.head.upstream.is_some())
    {
        counts.push_hint = "no outgoing commits or tags".into();
    }
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
            widgets::text_button(
                t,
                &counts.pull_label,
                None,
                enabled(Message::Pull).filter(|_| pull)
            ),
            counts.pull_hint.clone(),
        ),
        widgets::tip(
            t,
            widgets::text_button(
                t,
                &counts.push_label,
                None,
                enabled(Message::Push).filter(|_| push)
            ),
            counts.push_hint.clone(),
        ),
        dim(t, "│"),
        widgets::tip(
            t,
            widgets::text_button(
                t,
                "≡ stash",
                None,
                stash_hint.is_none().then_some(Message::Stash)
            ),
            stash_hint.unwrap_or("put your local changes aside").into(),
        ),
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
        let c = sync_counts(&head(Some("main"), Some("origin/main"), 2, 3), None, 0);
        assert_eq!(c.pull_label, "⇣ pull (3)");
        assert_eq!(c.push_label, "⇡ push (2)");
        assert!(c.pull_hint.contains("origin/main"));
        let c = sync_counts(&head(Some("main"), Some("origin/main"), 0, 0), None, 0);
        assert_eq!(
            (c.pull_label.as_str(), c.push_label.as_str()),
            ("⇣ pull", "⇡ push"),
            "zero counts are left out"
        );
    }

    #[test]
    fn unpublished_and_detached() {
        let c = sync_counts(&head(Some("topic"), None, 0, 0), Some(5), 0);
        assert_eq!(c.push_label, "⇡ push (5 new)");
        assert_eq!(c.pull_label, "⇣ pull (—)");
        let c = sync_counts(&head(None, None, 0, 0), None, 0);
        assert_eq!(c.push_label, "⇡ push (—)");
    }

    #[test]
    fn tags_the_remote_lacks_count_toward_push() {
        let up = head(Some("main"), Some("origin/main"), 0, 0);
        assert_eq!(sync_counts(&up, None, 1).push_label, "⇡ push (1 tag)");
        let up = head(Some("main"), Some("origin/main"), 2, 0);
        assert_eq!(sync_counts(&up, None, 3).push_label, "⇡ push (2 +3 tags)");
        let new = head(Some("topic"), None, 0, 0);
        assert_eq!(
            sync_counts(&new, Some(4), 1).push_label,
            "⇡ push (4 new +1 tag)"
        );
        assert!(sync_counts(&new, Some(4), 1).push_hint.contains("1 tag(s)"));
    }

    #[test]
    fn keeps_sync_available_for_incoming_commits_and_unpushed_tags() {
        let mut snapshot = Snapshot::default();
        snapshot.status.head = head(Some("main"), Some("origin/main"), 0, 0);
        snapshot.status.head.oid = Some(
            "1111111111111111111111111111111111111111"
                .parse()
                .expect("oid"),
        );
        assert_eq!(
            sync_available(&snapshot),
            (true, false),
            "the remote has not been fetched"
        );
        snapshot.last_fetch = Some(std::time::SystemTime::UNIX_EPOCH);
        assert_eq!(sync_available(&snapshot), (false, false));
        snapshot.status.head.behind = 2;
        snapshot.pending_tags = Some(crate::repo_tab::PendingTags {
            remote: "origin".into(),
            names: vec!["release-next".into()],
        });
        assert_eq!(sync_available(&snapshot), (true, true));
    }

    #[test]
    fn unknown_remote_tags_are_explained() {
        assert_eq!(unknown_tags_hint(&[]), "");
        assert!(unknown_tags_hint(&["origin".into()]).contains("`origin`"));
        assert!(unknown_tags_hint(&["a".into(), "b".into()]).contains("2 remotes"));
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
