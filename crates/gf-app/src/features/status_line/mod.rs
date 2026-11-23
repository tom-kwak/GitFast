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

/// Work running in the background that is not tied to a tab (a clone):
/// what it is doing, with a cancel button.
#[derive(Debug, Clone)]
pub struct Running<M> {
    pub text: String,
    pub on_cancel: M,
}

/// The undo / redo buttons: what the next undo and redo would do.
#[derive(Debug, Clone)]
pub struct UndoBar<M> {
    pub undo: Option<String>,
    pub redo: Option<String>,
    pub on_undo: M,
    pub on_redo: M,
    /// Opens the undo history.
    pub on_list: M,
}

pub fn view<'a, M: Clone + 'a>(
    t: &'a Tokens,
    tab: Option<&'a RepoTab>,
    busy: Option<(&'static str, Option<&str>)>,
    running: Option<Running<M>>,
    flash: Option<&'a Flash>,
    undo: Option<UndoBar<M>>,
) -> Element<'a, M> {
    let sep = || dim(t, "│");
    let mut left = Row::new().spacing(10).align_y(iced::Alignment::Center);

    match tab {
        None => left = left.push(dim(t, "no repository")),
        Some(tab) => {
            if let Some(err) = &tab.error {
                left = left.push(
                    colored(
                        t,
                        format!("⚠ {}", widgets::format::truncate(err, 80)),
                        t.error(),
                    )
                    .wrapping(iced::widget::text::Wrapping::None),
                );
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
    if let Some((frame, target)) = busy {
        let text = target.map_or_else(
            || format!("{frame} working"),
            |target| {
                format!(
                    "{frame} Switching to {}…",
                    widgets::format::truncate(target, 32)
                )
            },
        );
        right =
            right.push(colored(t, text, t.warning()).wrapping(iced::widget::text::Wrapping::None));
    }
    if let Some(r) = running {
        right = right.push(
            row![
                colored(t, widgets::format::truncate(&r.text, 70), t.warning())
                    .wrapping(iced::widget::text::Wrapping::None),
                widgets::text_button(t, "cancel", Some("Esc"), Some(r.on_cancel)),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center),
        );
    }
    if let Some(f) = flash {
        let color = match f.kind {
            NoteKind::Info => t.success(),
            NoteKind::Error => t.error(),
        };
        // One line, always: a wrapped flash would make the bar taller and
        // move every button above it while the user aims at one.
        right = right.push(
            colored(t, widgets::format::truncate(&f.text, 60), color)
                .wrapping(iced::widget::text::Wrapping::None),
        );
    }
    let undo = undo.filter(|b| b.undo.is_some() || b.redo.is_some());
    // The key hints make room for the undo buttons (the prompt shows the
    // same hints).
    let hints = undo.is_none();
    if let Some(bar) = undo {
        let mut buttons = Row::new().spacing(2).align_y(iced::Alignment::Center);
        if let Some(label) = &bar.undo {
            buttons = buttons.push(widgets::tip(
                t,
                widgets::text_button(
                    t,
                    &format!("↶ {}", widgets::format::truncate(label, 28)),
                    Some("⌘Z"),
                    Some(bar.on_undo.clone()),
                ),
                format!("undo \"{label}\" (also `:undo`)"),
            ));
        }
        if let Some(label) = &bar.redo {
            buttons = buttons.push(widgets::tip(
                t,
                widgets::text_button(t, "↷", Some("⌘⇧Z"), Some(bar.on_redo.clone())),
                format!("redo \"{label}\" (also `:redo`)"),
            ));
        }
        buttons = buttons.push(widgets::tip(
            t,
            widgets::text_button(t, "▾", None, Some(bar.on_list.clone())),
            "undo history — go back several steps (also `:undo list`)".into(),
        ));
        right = right.push(buttons);
    }
    if hints {
        right = right.push(
            dim(t, "⌘O open · ⌘R refresh · ⌘L prompt · ⌘J log · ? help")
                .wrapping(iced::widget::text::Wrapping::None),
        );
    }

    container(
        row![left, space().width(Length::Fill), right]
            .spacing(10)
            .align_y(iced::Alignment::Center),
    )
    .padding(Padding::from([3, 8]))
    .width(Length::Fill)
    .clip(true)
    .style(style::bar(t))
    .into()
}
