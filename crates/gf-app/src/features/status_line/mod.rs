//! Bottom status line (tmux/powerline style): branch and upstream state,
//! change counts, activity indicator, the latest result, and key hints.

use gf_core::model::StatusEntry;
use iced::widget::{Row, container, row, space};
use iced::{Element, Length, Padding};

use crate::features::command_log::NoteKind;
use crate::repo_tab::RepoTab;
use crate::theme::{Tokens, style};
use crate::widgets::{self, colored, dim};

/// Short-lived result shown in the status line after an action.
#[derive(Debug, Clone)]
pub struct Flash {
    pub kind: NoteKind,
    pub text: String,
}

pub fn view<'a, M: 'a>(
    t: &'a Tokens,
    tab: Option<&'a RepoTab>,
    busy: Option<&'static str>,
    flash: Option<&'a Flash>,
) -> Element<'a, M> {
    let sep = || dim(t, "│");
    let mut left = Row::new().spacing(10).align_y(iced::Alignment::Center);

    match tab {
        None => left = left.push(dim(t, "no repository")),
        Some(tab) => {
            if let Some(err) = &tab.error {
                left = left.push(colored(
                    t,
                    format!("⚠ {}", widgets::format::truncate(err, 80)),
                    t.error(),
                ));
            }
            if let Some(snap) = &tab.snapshot {
                let head = &snap.status.head;
                let branch = tab.head_label().unwrap_or_default();
                let mut b = row![widgets::segment(t, "⎇", branch, t.branch())].spacing(6);
                if let Some(up) = &head.upstream {
                    b = b.push(dim(t, format!("→ {up}")));
                    if head.ahead > 0 {
                        b = b.push(colored(t, format!("↑{}", head.ahead), t.success()));
                    }
                    if head.behind > 0 {
                        b = b.push(colored(t, format!("↓{}", head.behind), t.warning()));
                    }
                }
                left = left.push(b).push(sep());

                let (mut staged, mut modified, mut untracked, mut conflicts) = (0, 0, 0, 0);
                for e in &snap.status.entries {
                    match e {
                        StatusEntry::Conflicted { .. } => conflicts += 1,
                        StatusEntry::Untracked { .. } => untracked += 1,
                        StatusEntry::Ignored { .. } => {}
                        StatusEntry::Changed { .. } => {
                            if e.is_staged() {
                                staged += 1;
                            }
                            if e.has_unstaged_changes() {
                                modified += 1;
                            }
                        }
                    }
                }
                if staged + modified + untracked + conflicts == 0 {
                    left = left.push(colored(t, "✓ clean", t.success()));
                } else {
                    if conflicts > 0 {
                        left = left.push(colored(t, format!("✗ {conflicts} conflicts"), t.error()));
                    }
                    if staged > 0 {
                        left = left.push(colored(t, format!("● {staged} staged"), t.added()));
                    }
                    if modified > 0 {
                        left = left.push(colored(t, format!("✚ {modified} modified"), t.removed()));
                    }
                    if untracked > 0 {
                        left = left.push(dim(t, format!("? {untracked} untracked")));
                    }
                }
            }
        }
    }

    let mut right = Row::new().spacing(10).align_y(iced::Alignment::Center);
    if let Some(frame) = busy {
        right = right.push(colored(t, format!("{frame} working"), t.warning()));
    }
    if let Some(f) = flash {
        let color = match f.kind {
            NoteKind::Info => t.success(),
            NoteKind::Error => t.error(),
        };
        right = right.push(colored(t, widgets::format::truncate(&f.text, 60), color));
    }
    right = right.push(dim(t, "⌘O open · ⌘R refresh · ⌘L prompt · ⌘J log · ? help"));

    container(
        row![left, space().width(Length::Fill), right]
            .spacing(10)
            .align_y(iced::Alignment::Center),
    )
    .padding(Padding::from([3, 8]))
    .width(Length::Fill)
    .style(style::bar(t))
    .into()
}
