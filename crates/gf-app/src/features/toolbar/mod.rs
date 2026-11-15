//! Repository toolbar: fetch / pull / push with live counts and progress,
//! plus quick branch, stash and refresh actions.

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
    NewBranch,
    Stash,
    Refresh,
    Cancel,
    ToggleRefs,
}

/// Which side panels are open (shown as toggle buttons).
#[derive(Debug, Clone, Copy)]
pub struct Panels {
    pub refs: bool,
}

/// A running network operation.
#[derive(Debug, Clone)]
pub struct Sync {
    pub label: String,
    pub progress: Option<Progress>,
}

pub fn view<'a>(
    t: &'a Tokens,
    tab: &'a RepoTab,
    sync: Option<&'a Sync>,
    spinner: &'a str,
    panels: Panels,
) -> Element<'a, Message> {
    let head = tab.snapshot.as_ref().map(|s| &s.status.head);
    let (ahead, behind) = head.map_or((0, 0), |h| (h.ahead, h.behind));
    let has_remote = tab.snapshot.as_ref().is_some_and(|s| !s.remotes.is_empty());
    let busy = sync.is_some();
    let enabled = |m: Message| (!busy && has_remote).then_some(m);

    let pull_label = if behind > 0 {
        format!("⇣ pull {behind}")
    } else {
        "⇣ pull".into()
    };
    let push_label = if ahead > 0 {
        format!("⇡ push {ahead}")
    } else {
        "⇡ push".into()
    };

    let mut bar = row![
        widgets::tip(
            t,
            widgets::text_button(t, "⟳ fetch", None, enabled(Message::Fetch)),
            "download new commits from all remotes (does not change your files)".into()
        ),
        widgets::tip(
            t,
            widgets::text_button(t, &pull_label, None, enabled(Message::Pull)),
            "fetch and integrate the upstream branch".into()
        ),
        widgets::tip(
            t,
            widgets::text_button(t, &push_label, None, enabled(Message::Push)),
            "upload your commits to the upstream branch".into()
        ),
        dim(t, "│"),
        widgets::text_button(t, "+ branch", None, Some(Message::NewBranch)),
        widgets::text_button(t, "≡ stash", None, Some(Message::Stash)),
        widgets::text_button(t, "↻ refresh", Some("⌘R"), Some(Message::Refresh)),
        dim(t, "│"),
        widgets::tip(
            t,
            widgets::text_button(
                t,
                if panels.refs { "■ refs" } else { "□ refs" },
                Some("⌘B"),
                Some(Message::ToggleRefs)
            ),
            "show/hide the branches · tags · stashes sidebar".into(),
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

    container(bar)
        .padding(Padding::from([2, 4]))
        .width(Length::Fill)
        .style(style::bar(t))
        .into()
}
