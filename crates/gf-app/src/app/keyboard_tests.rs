//! Regression tests for the keyboard: how one key press is routed between
//! the panels, the lists that open over the window (tabs, undo history, the
//! repository picker), menus and dialogs (`focus.rs`).
//!
//! Keys go through `Message::KeyInput`, as the event listener sends them,
//! so what is pinned here is what the user's key presses do.

use std::path::PathBuf;

use iced::keyboard;

use super::ui_sweep::{Fixture, Ui};
use super::*;
use crate::features::undo_list;
use crate::nav::{Key, Nav, Pane};

fn input(shortcut: Option<Shortcut>, nav: Option<Nav>) -> Message {
    Message::KeyInput(focus::KeyInput {
        shortcut,
        menu: None,
        nav,
    })
}

/// A navigation key (↑, ⇥, ⏎, `/` ...) without modifiers.
async fn press(ui: &mut Ui, key: Key) {
    let _ = ui.send(input(None, Some(Nav::plain(key)))).await;
}

/// ⇧ + a navigation key.
async fn press_shift(ui: &mut Ui, key: Key) {
    let nav = Nav { key, shift: true };
    let _ = ui.send(input(None, Some(nav))).await;
}

/// A shortcut (⌘P, Esc ...).
async fn chord(ui: &mut Ui, shortcut: Shortcut) {
    let _ = ui.send(input(Some(shortcut), None)).await;
}

/// The fixture plus a second repository in another tab; the first is shown.
async fn two_tabs() -> (Ui, PathBuf) {
    let mut ui = Ui::open().await;
    let other = Fixture::new();
    let path = other.repo.path().to_path_buf();
    ui.others.push(other);
    let _ = ui.send(Message::Open(path.clone())).await;
    let _ = ui.send(Message::Tabs(tabs::Message::Select(0))).await;
    assert_eq!((ui.app.tabs.len(), ui.app.active), (2, 0));
    (ui, path)
}

fn dialog_title(ui: &Ui) -> Option<&str> {
    ui.app.dialog.as_ref().map(|d| d.title.as_str())
}

// ------------------------------------------------------------ panels

#[tokio::test]
async fn tab_moves_through_the_panels_and_shift_tab_goes_back() {
    let mut ui = Ui::open().await;
    let panes = ui.app.panes();
    assert!(panes.len() > 2, "{panes:?}");
    let start = ui.app.pane();
    let at = panes.iter().position(|p| *p == start).expect("shown pane");

    press(&mut ui, Key::Tab).await;
    assert_eq!(ui.app.pane(), panes[(at + 1) % panes.len()]);
    press_shift(&mut ui, Key::Tab).await;
    assert_eq!(ui.app.pane(), start);
    press_shift(&mut ui, Key::Tab).await;
    assert_eq!(
        ui.app.pane(),
        panes[(at + panes.len() - 1) % panes.len()],
        "⇧⇥ wraps from the first panel to the last"
    );
}

#[tokio::test]
async fn tab_wraps_after_the_last_panel() {
    let mut ui = Ui::open().await;
    let n = ui.app.panes().len();
    let start = ui.app.pane();
    for _ in 0..n {
        press(&mut ui, Key::Tab).await;
    }
    assert_eq!(ui.app.pane(), start, "a full turn comes back");
}

#[tokio::test]
async fn clicking_a_panel_makes_it_the_one_the_keys_drive() {
    let mut ui = Ui::open().await;
    let _ = ui.send(Message::FocusPane(Pane::Sidebar)).await;
    assert_eq!(ui.app.pane(), Pane::Sidebar);
    let _ = ui.send(Message::FocusPane(Pane::Diff)).await;
    assert_eq!(ui.app.pane(), Pane::Diff);
}

#[tokio::test]
async fn a_focus_on_a_hidden_panel_falls_back_to_a_shown_one() {
    let mut ui = Ui::open().await;
    ui.app.focus = Pane::Changes; // the working tree is not shown in history mode
    assert!(!ui.app.panes().contains(&Pane::Changes));
    assert!(ui.app.panes().contains(&ui.app.pane()));
}

#[tokio::test]
async fn slash_goes_to_the_prompt_and_escape_leaves_it() {
    let mut ui = Ui::open().await;
    assert!(!ui.app.typing);

    press(&mut ui, Key::Char('/')).await;
    assert!(ui.app.typing, "keys now type into the prompt");

    ui.app.focus_pane(Pane::History);
    assert!(!ui.app.typing, "choosing a panel leaves the prompt");
}

#[tokio::test]
async fn the_focus_prompt_shortcut_starts_typing() {
    let mut ui = Ui::open().await;
    chord(&mut ui, Shortcut::FocusPrompt).await;
    assert!(ui.app.typing);
}

// ---------------------------------------------------------- tab list

#[tokio::test]
async fn the_tab_list_opens_with_its_shortcut_and_closes_with_escape() {
    let (mut ui, _) = two_tabs().await;
    assert!(ui.app.tab_list.is_none());

    chord(&mut ui, Shortcut::TabList).await;
    assert!(ui.app.tab_list.is_some());
    chord(&mut ui, Shortcut::Escape).await;
    assert!(ui.app.tab_list.is_none());
}

#[tokio::test]
async fn the_tab_list_shortcut_toggles_it() {
    let (mut ui, _) = two_tabs().await;
    chord(&mut ui, Shortcut::TabList).await;
    chord(&mut ui, Shortcut::TabList).await;
    assert!(ui.app.tab_list.is_none());
}

#[tokio::test]
async fn arrows_move_in_the_tab_list_and_enter_shows_that_tab() {
    let (mut ui, _) = two_tabs().await;
    chord(&mut ui, Shortcut::TabList).await;
    let first = ui.app.tab_list.as_ref().and_then(|l| l.selected());

    press(&mut ui, Key::Down).await;
    let second = ui.app.tab_list.as_ref().and_then(|l| l.selected());
    assert_ne!(first, second, "↓ moves the highlight");
    press(&mut ui, Key::Up).await;
    assert_eq!(ui.app.tab_list.as_ref().and_then(|l| l.selected()), first);

    press(&mut ui, Key::Down).await;
    press(&mut ui, Key::Enter).await;
    assert!(ui.app.tab_list.is_none(), "⏎ closes the list");
    assert_eq!(Some(ui.app.active), second, "and shows the tab it was on");
}

#[tokio::test]
async fn a_number_picks_that_tab_from_the_list() {
    let (mut ui, _) = two_tabs().await;
    chord(&mut ui, Shortcut::TabList).await;
    chord(&mut ui, Shortcut::SelectTab(1)).await;
    assert_eq!(ui.app.active, 1);
    assert!(ui.app.tab_list.is_none());
}

#[tokio::test]
async fn close_tab_in_the_list_closes_the_highlighted_tab() {
    let (mut ui, _) = two_tabs().await;
    chord(&mut ui, Shortcut::TabList).await;
    press(&mut ui, Key::Down).await;
    assert_eq!(ui.app.tab_list.as_ref().and_then(|l| l.selected()), Some(1));

    chord(&mut ui, Shortcut::CloseTab).await;
    assert_eq!(ui.app.tabs.len(), 1, "the highlighted tab was closed");
    assert_eq!(ui.app.active, 0, "the other one stays");
    assert!(
        ui.app.tab_list.is_none(),
        "one tab left: nothing to choose from"
    );
}

#[tokio::test]
async fn close_tab_in_the_list_keeps_it_open_while_tabs_remain() {
    let (mut ui, _) = two_tabs().await;
    let third = Fixture::new();
    let path = third.repo.path().to_path_buf();
    ui.others.push(third);
    let _ = ui.send(Message::Open(path)).await;
    let _ = ui.send(Message::Tabs(tabs::Message::Select(0))).await;
    assert_eq!(ui.app.tabs.len(), 3);

    chord(&mut ui, Shortcut::TabList).await;
    chord(&mut ui, Shortcut::CloseTab).await;
    assert_eq!(ui.app.tabs.len(), 2);
    assert!(ui.app.tab_list.is_some(), "the list stays to close more");
}

#[tokio::test]
async fn escape_in_the_tab_list_first_clears_its_filter() {
    let (mut ui, _) = two_tabs().await;
    chord(&mut ui, Shortcut::TabList).await;
    let _ = ui
        .send(Message::TabList(tab_list::Message::Filter("zzz".into())))
        .await;

    chord(&mut ui, Shortcut::Escape).await;
    assert!(
        ui.app.tab_list.is_some(),
        "the first Esc only clears the filter"
    );
    chord(&mut ui, Shortcut::Escape).await;
    assert!(ui.app.tab_list.is_none(), "the second closes the list");
}

// ---------------------------------------------------------- switcher

#[tokio::test]
async fn the_tab_switcher_moves_with_the_key_and_enter_switches() {
    let (mut ui, _) = two_tabs().await;
    let _ = ui.send(Message::Modifiers(keyboard::Modifiers::CTRL)).await;

    chord(&mut ui, Shortcut::CycleTab { back: false }).await;
    assert!(ui.app.switcher.is_some());
    press(&mut ui, Key::Enter).await;

    assert!(ui.app.switcher.is_none());
    assert_eq!(ui.app.active, 1, "the other tab is shown");
}

#[tokio::test]
async fn escape_cancels_the_tab_switcher_without_switching() {
    let (mut ui, _) = two_tabs().await;
    let _ = ui.send(Message::Modifiers(keyboard::Modifiers::CTRL)).await;
    chord(&mut ui, Shortcut::CycleTab { back: false }).await;

    chord(&mut ui, Shortcut::Escape).await;
    assert!(ui.app.switcher.is_none());
    assert_eq!(ui.app.active, 0);
}

#[tokio::test]
async fn letting_go_of_control_switches_to_the_highlighted_tab() {
    let (mut ui, _) = two_tabs().await;
    let _ = ui.send(Message::Modifiers(keyboard::Modifiers::CTRL)).await;
    chord(&mut ui, Shortcut::CycleTab { back: false }).await;

    let _ = ui
        .send(Message::Modifiers(keyboard::Modifiers::empty()))
        .await;
    assert!(ui.app.switcher.is_none());
    assert_eq!(ui.app.active, 1);
}

#[tokio::test]
async fn the_switcher_with_a_single_tab_only_says_so() {
    let mut ui = Ui::open().await;
    let _ = ui.send(Message::Modifiers(keyboard::Modifiers::CTRL)).await;
    chord(&mut ui, Shortcut::CycleTab { back: false }).await;
    assert!(ui.app.switcher.is_none());
    assert!(ui.flash().contains("only one tab"), "{}", ui.flash());
}

#[tokio::test]
async fn previous_and_next_tab_shortcuts_wrap_around() {
    let (mut ui, _) = two_tabs().await;
    chord(&mut ui, Shortcut::NextTab).await;
    assert_eq!(ui.app.active, 1);
    chord(&mut ui, Shortcut::NextTab).await;
    assert_eq!(ui.app.active, 0, "wraps to the first");
    chord(&mut ui, Shortcut::PrevTab).await;
    assert_eq!(ui.app.active, 1, "wraps to the last");
}

// ---------------------------------------------------------- undo list

/// Discards the fixture's untracked `c.txt`: one step in the undo history.
async fn make_an_undo_step(ui: &mut Ui) {
    let _ = ui
        .send(Message::Changes(changes::Message::Discard(
            "c.txt".into(),
            changes::Group::Untracked,
        )))
        .await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
}

#[tokio::test]
async fn the_undo_history_does_not_open_when_there_is_nothing_to_undo() {
    let mut ui = Ui::open().await;
    let _ = ui.send(Message::UndoList(undo_list::Message::Toggle)).await;
    assert!(ui.app.undo_list.is_none());
    assert!(ui.flash().contains("nothing to undo"), "{}", ui.flash());
}

#[tokio::test]
async fn escape_closes_the_undo_history() {
    let mut ui = Ui::open().await;
    make_an_undo_step(&mut ui).await;
    let _ = ui.send(Message::UndoList(undo_list::Message::Toggle)).await;
    assert!(ui.app.undo_list.is_some());

    chord(&mut ui, Shortcut::Escape).await;
    assert!(ui.app.undo_list.is_none());
}

#[tokio::test]
async fn enter_in_the_undo_history_goes_back_to_the_highlighted_step() {
    let mut ui = Ui::open().await;
    make_an_undo_step(&mut ui).await;
    assert!(!ui.fixture.repo.path().join("c.txt").exists());
    let _ = ui.send(Message::UndoList(undo_list::Message::Toggle)).await;

    press(&mut ui, Key::Enter).await;
    assert!(ui.app.undo_list.is_none());
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert!(
        ui.fixture.repo.path().join("c.txt").exists(),
        "the step was undone"
    );
}

#[tokio::test]
async fn the_undo_history_keys_move_the_highlight_and_stop_at_the_ends() {
    let mut ui = Ui::open().await;
    for i in 0..3 {
        ui.fixture.repo.write(format!("u{i}.txt"), "x\n");
        let name = format!("u{i}.txt");
        let _ = ui
            .send(Message::Changes(changes::Message::Discard(
                name.into(),
                changes::Group::Untracked,
            )))
            .await;
        let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    }
    let _ = ui.send(Message::UndoList(undo_list::Message::Toggle)).await;
    let rows = undo_list::rows(&ui.app.tabs[0].undo);
    assert!(rows.len() >= 3, "{rows:?}");
    let at = |ui: &Ui| {
        ui.app
            .undo_list
            .as_ref()
            .map(|l| l.current(&undo_list::rows(&ui.app.tabs[0].undo)))
            .expect("open")
    };

    let start = at(&ui);
    press(&mut ui, Key::Down).await;
    let down = at(&ui);
    press(&mut ui, Key::Up).await;
    assert_eq!(at(&ui), start, "↑ undoes ↓ (down was {down})");
    press(&mut ui, Key::PageDown).await;
    assert_eq!(at(&ui), rows.len() - 1, "PgDn goes to the last row");
    press(&mut ui, Key::PageUp).await;
    assert_eq!(at(&ui), 0, "PgUp goes to the first row");
}

#[tokio::test]
async fn the_undo_shortcut_closes_the_history_and_undoes_one_step() {
    let mut ui = Ui::open().await;
    make_an_undo_step(&mut ui).await;
    assert!(!ui.fixture.repo.path().join("c.txt").exists());

    let _ = ui.send(Message::UndoList(undo_list::Message::Toggle)).await;
    chord(&mut ui, Shortcut::Undo).await;
    assert!(ui.app.undo_list.is_none());
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert!(
        ui.fixture.repo.path().join("c.txt").exists(),
        "the discard was undone"
    );
}

// ----------------------------------------------------- repository picker

#[tokio::test]
async fn the_open_shortcut_shows_the_picker_and_escape_closes_it() {
    let mut ui = Ui::welcome().await;
    chord(&mut ui, Shortcut::Open).await;
    assert!(ui.app.picker.is_some());

    chord(&mut ui, Shortcut::Escape).await;
    assert!(ui.app.picker.is_none());
}

#[tokio::test]
async fn enter_in_the_picker_opens_the_highlighted_repository() {
    let mut ui = Ui::welcome().await;
    chord(&mut ui, Shortcut::Open).await;

    press(&mut ui, Key::Enter).await;
    assert_eq!(
        ui.app.tabs.len(),
        1,
        "the recent repository opened in a tab"
    );
    assert!(ui.app.picker.is_none());
}

#[tokio::test]
async fn a_number_in_the_picker_opens_that_row() {
    let mut ui = Ui::welcome().await;
    chord(&mut ui, Shortcut::Open).await;

    chord(&mut ui, Shortcut::SelectTab(0)).await;
    assert_eq!(ui.app.tabs.len(), 1);
}

#[tokio::test]
async fn arrows_in_the_picker_do_not_open_anything() {
    let mut ui = Ui::welcome().await;
    chord(&mut ui, Shortcut::Open).await;
    press(&mut ui, Key::Down).await;
    press(&mut ui, Key::Up).await;
    assert!(ui.app.picker.is_some());
    assert!(ui.app.tabs.is_empty());
}

// ---------------------------------------------------------- settings

#[tokio::test]
async fn arrows_in_the_settings_cycle_the_theme_and_escape_closes_them() {
    let mut ui = Ui::welcome().await;
    chord(&mut ui, Shortcut::Settings).await;
    assert!(ui.app.settings_open);
    let before = ui.app.tokens.name;

    press(&mut ui, Key::Down).await;
    let next = ui.app.tokens.name;
    assert_ne!(next, before, "↓ picks the next theme");
    press(&mut ui, Key::Up).await;
    assert_eq!(ui.app.tokens.name, before, "↑ goes back");

    chord(&mut ui, Shortcut::Escape).await;
    assert!(!ui.app.settings_open);
}

// ---------------------------------------------------------- dialogs

#[tokio::test]
async fn enter_confirms_a_dialog_and_escape_cancels_it() {
    let mut ui = Ui::open_with(|repo| {
        repo.git(&["reset", "-q", "--hard"]);
        repo.git(&["clean", "-fdq"]);
    })
    .await;
    let third = ui.fixture.repo.head();
    let oid: gf_core::model::Oid = third.parse().expect("oid");

    let act = |a| Message::History(history::Message::Act(a));
    let _ = ui
        .send(Message::History(history::Message::Select(oid)))
        .await;
    let _ = ui.send(act(history::CommitAction::Revert)).await;
    assert_eq!(dialog_title(&ui), Some("Revert"));
    chord(&mut ui, Shortcut::Escape).await;
    assert!(ui.app.dialog.is_none());
    assert_eq!(
        ui.fixture.repo.head(),
        third,
        "Esc did nothing to the repository"
    );

    let _ = ui.send(act(history::CommitAction::Revert)).await;
    chord(&mut ui, Shortcut::Enter).await;
    assert!(ui.app.dialog.is_none());
    assert_ne!(ui.fixture.repo.head(), third, "⏎ confirmed the revert");
}

#[tokio::test]
async fn panel_keys_do_nothing_behind_an_open_dialog() {
    let mut ui = Ui::open().await;
    let _ = ui.send(Message::Toolbar(toolbar::Message::Pull)).await;
    let _ = ui
        .send(Message::Sidebar(sidebar::Message::Act(
            sidebar::Action::NewTag,
        )))
        .await;
    assert_eq!(dialog_title(&ui), Some("New tag"));
    let pane = ui.app.pane();

    press(&mut ui, Key::Tab).await;
    assert_eq!(
        ui.app.pane(),
        pane,
        "⇥ does not move the panel under a dialog"
    );
    assert!(ui.app.dialog.is_some());
}

// ------------------------------------------------------------- menus

#[tokio::test]
async fn escape_closes_an_open_menu_and_other_shortcuts_wait() {
    let mut ui = Ui::open().await;
    let _ = ui.right_click("second").await;
    assert!(ui.app.menu.is_some());

    chord(&mut ui, Shortcut::TabList).await;
    assert!(
        ui.app.menu.is_some(),
        "other shortcuts wait until it closes"
    );
    assert!(ui.app.tab_list.is_none());

    chord(&mut ui, Shortcut::Escape).await;
    assert!(ui.app.menu.is_none());
}

#[tokio::test]
async fn arrows_and_enter_pick_an_item_of_an_open_menu() {
    let mut ui = Ui::open().await;
    let _ = ui.right_click("second").await;
    assert!(ui.app.menu.is_some());

    let menu = |key| {
        Message::KeyInput(focus::KeyInput {
            shortcut: None,
            menu: Some(key),
            nav: None,
        })
    };
    let _ = ui.send(menu(context_menu::Key::Down)).await;
    let _ = ui.send(menu(context_menu::Key::Up)).await;
    assert!(ui.app.menu.is_some(), "moving keeps it open");
    let _ = ui.send(menu(context_menu::Key::Enter)).await;
    assert!(
        ui.app.menu.is_none(),
        "⏎ picks the highlighted item and closes it"
    );
}

#[tokio::test]
async fn quit_works_from_the_welcome_screen_the_picker_and_the_settings() {
    let mut ui = Ui::welcome().await;
    let fx = ui.send(input(Some(Shortcut::Quit), None)).await;
    assert!(fx.window > 0, "quit from the welcome screen");

    chord(&mut ui, Shortcut::Open).await;
    let fx = ui.send(input(Some(Shortcut::Quit), None)).await;
    assert!(fx.window > 0, "quit from the picker");

    let mut ui = Ui::welcome().await;
    chord(&mut ui, Shortcut::Settings).await;
    let fx = ui.send(input(Some(Shortcut::Quit), None)).await;
    assert!(fx.window > 0, "quit from the settings");
}
