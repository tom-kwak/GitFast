//! Banner shown while a multi-step operation is in progress (merge,
//! rebase, cherry-pick, revert, bisect): what is happening, how many
//! conflicts are left, and the buttons to continue, skip or abort — so the
//! user is never stuck in a half-finished state without knowing it.

use gf_git::{BisectProgress, RepoState};
use iced::widget::{container, row, space};
use iced::{Background, Border, Element, Length, Padding};

use crate::theme::{Tokens, style};
use crate::widgets::{self, colored, dim};

#[derive(Debug, Clone)]
pub enum Message {
    Continue,
    Skip,
    Abort,
    Bisect(gf_git::Verdict),
    BisectReset,
    ShowConflicts,
    /// Pop the stash of changes gf left on this branch.
    PopLeft(usize),
    /// Hide that offer (the stash stays in the list).
    DismissLeft(gf_core::model::Oid),
}

/// Offer to bring back the changes gf stashed when the user switched away
/// from this branch ("stash, then switch").
pub fn left_changes<'a>(t: &'a Tokens, stash: &gf_core::model::StashEntry) -> Element<'a, Message> {
    let bar = row![
        colored(t, "≡", t.accent()),
        widgets::mono(
            t,
            format!(
                "changes you left here are in stash@{{{}}} ({})",
                stash.index,
                crate::undo::age(u64::try_from(stash.time.seconds).unwrap_or(0))
            )
        ),
        space().width(Length::Fill),
        widgets::text_button(t, "pop", None, Some(Message::PopLeft(stash.index))),
        widgets::text_button(
            t,
            "dismiss",
            None,
            Some(Message::DismissLeft(stash.id.clone()))
        ),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);
    container(bar)
        .padding(Padding::from([3, 8]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            text_color: Some(t.fg),
            background: Some(Background::Color(style::tint(t.bg, t.accent(), 0.12))),
            border: Border {
                color: t.accent(),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

pub fn view<'a>(
    t: &'a Tokens,
    state: &'a RepoState,
    conflicts: usize,
    bisect: Option<&'a BisectProgress>,
) -> Option<Element<'a, Message>> {
    if state.is_clean() {
        return None;
    }
    let mut bar = row![colored(t, format!("⚠ {}", state.label()), t.warning())]
        .spacing(8)
        .align_y(iced::Alignment::Center);

    if *state == RepoState::Bisecting {
        let progress = match bisect {
            Some(BisectProgress {
                first_bad: Some(id),
                ..
            }) => format!("found: {} is the first bad commit", id.short()),
            Some(BisectProgress {
                remaining: Some(n),
                steps,
                ..
            }) => format!(
                "test the checked-out commit · {n} left (~{} steps)",
                steps.unwrap_or(0)
            ),
            _ => "test the checked-out commit, then mark it".into(),
        };
        bar = bar
            .push(dim(t, progress))
            .push(space().width(Length::Fill))
            .push(widgets::text_button(
                t,
                "good",
                None,
                Some(Message::Bisect(gf_git::Verdict::Good)),
            ))
            .push(widgets::text_button(
                t,
                "bad",
                None,
                Some(Message::Bisect(gf_git::Verdict::Bad)),
            ))
            .push(widgets::text_button(
                t,
                "skip",
                None,
                Some(Message::Bisect(gf_git::Verdict::Skip)),
            ))
            .push(widgets::text_button(
                t,
                "end bisect",
                None,
                Some(Message::BisectReset),
            ));
    } else {
        let detail = match state {
            RepoState::Merging { message: Some(m) } => m.clone(),
            RepoState::Rebasing {
                branch: Some(b), ..
            } => format!("rebasing `{b}`"),
            _ => String::new(),
        };
        bar = bar.push(dim(t, detail));
        if conflicts > 0 {
            bar = bar.push(widgets::text_button(
                t,
                &format!(
                    "{conflicts} conflict{} to resolve",
                    if conflicts == 1 { "" } else { "s" }
                ),
                None,
                Some(Message::ShowConflicts),
            ));
        } else {
            bar = bar.push(colored(t, "✓ no conflicts left", t.success()));
        }
        bar = bar.push(space().width(Length::Fill));
        let continue_btn = widgets::text_button(
            t,
            "continue",
            None,
            (conflicts == 0).then_some(Message::Continue),
        );
        bar = bar.push(if conflicts == 0 {
            continue_btn
        } else {
            widgets::tip(
                t,
                continue_btn,
                "resolve and stage every conflicted file first".into(),
            )
        });
        if state.can_skip() {
            bar = bar.push(widgets::danger_button(
                t,
                "skip commit",
                Some(Message::Skip),
            ));
        }
        bar = bar.push(widgets::danger_button(t, "abort", Some(Message::Abort)));
    }

    Some(
        container(bar)
            .padding(Padding::from([3, 8]))
            .width(Length::Fill)
            .style(move |_| container::Style {
                text_color: Some(t.fg),
                background: Some(Background::Color(style::tint(t.bg, t.warning(), 0.15))),
                border: Border {
                    color: t.warning(),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..container::Style::default()
            })
            .into(),
    )
}
