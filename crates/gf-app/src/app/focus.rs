//! Keyboard focus: which panel the arrow keys drive, and how one key press
//! is routed between an open menu, a dialog, a text field and the panels.
//!
//! - A text field with focus keeps its keys (the prompt also gets ↑/↓
//!   history and ⇥ completion). Esc leaves it.
//! - Otherwise keys go to the active [`Pane`]; what it does not use, the
//!   app uses: ←/→ (and Enter) move between panels, ⇥ cycles them, `/`
//!   goes to the prompt.
//! - Clicking a panel makes it active.

use iced::widget::{container, mouse_area, operation};
use iced::{Border, Element, Task};

use super::{App, Message, Shortcut};
use crate::features::context_menu;
use crate::features::diff_view::Role;
use crate::features::history::{self, search};
use crate::features::prompt::PROMPT_ID;
use crate::features::sidebar::REFS_FILTER_ID;
use crate::features::{repo_picker, tab_list};
use crate::nav::{Key, Moved, Nav, Pane};
use crate::theme::Tokens;
use crate::widgets::pointer::Field;

/// One key press, as each possible receiver reads it.
#[derive(Debug, Clone)]
pub struct KeyInput {
    pub shortcut: Option<Shortcut>,
    pub menu: Option<context_menu::Key>,
    pub nav: Option<Nav>,
}

/// Shortcuts that only make sense inside the prompt.
/// Keys a focused text field handles itself (copy, select all).
fn text_edit(s: Shortcut) -> bool {
    matches!(s, Shortcut::Copy | Shortcut::SelectAll)
}

fn prompt_only(s: Shortcut) -> bool {
    matches!(
        s,
        Shortcut::HistoryPrev | Shortcut::HistoryNext | Shortcut::Complete
    )
}

impl App {
    /// Panels on screen, left to right.
    pub(super) fn panes(&self) -> Vec<Pane> {
        let Some(tab) = self.active_tab() else {
            return Vec::new();
        };
        let mut panes = Vec::new();
        if self.settings.appearance.show_refs {
            panes.push(Pane::Sidebar);
        }
        match tab.code_view {
            Role::Commit => {
                if self.settings.appearance.show_history {
                    panes.push(Pane::History);
                }
                if tab.blame.is_none() {
                    panes.push(Pane::Files);
                }
            }
            Role::Local => panes.push(Pane::Changes),
        }
        panes.push(Pane::Diff);
        panes
    }

    /// The active panel; falls back to the list of the current mode when
    /// the remembered one is not on screen.
    pub(super) fn pane(&self) -> Pane {
        if self.panes().contains(&self.focus) {
            return self.focus;
        }
        match self.active_tab().map(|t| t.code_view) {
            Some(Role::Local) => Pane::Changes,
            Some(Role::Commit) if !self.settings.appearance.show_history => {
                if self.active_tab().is_some_and(|tab| tab.blame.is_some()) {
                    Pane::Diff
                } else {
                    Pane::Files
                }
            }
            _ => Pane::History,
        }
    }

    pub(super) fn focus_pane(&mut self, pane: Pane) {
        self.focus = pane;
        self.typing = false;
    }

    /// After a dialog, menu or command: back to the prompt only if the user
    /// was typing there (or there is nothing else to focus).
    pub(super) fn restore_focus(&self) -> Task<Message> {
        if self.settings_open || self.code_dialog.is_some() || self.dialog.is_some() {
            return Task::none();
        }
        if self.picker.is_some() {
            return operation::focus(repo_picker::FILTER_ID);
        }
        if self
            .active_tab()
            .is_some_and(|tab| tab.history.file_search_is_open())
        {
            return operation::focus(search::INPUT_ID);
        }
        if self.tab_list.is_some() {
            return operation::focus(tab_list::FILTER_ID);
        }
        if self.typing || self.tabs.is_empty() {
            operation::focus(PROMPT_ID)
        } else {
            Task::none()
        }
    }

    pub(super) fn focus_prompt(&mut self) -> Task<Message> {
        self.typing = true;
        operation::focus(PROMPT_ID)
    }

    pub(super) fn on_key_input(&mut self, k: KeyInput) -> Task<Message> {
        fn run(app: &mut App, s: Option<Shortcut>) -> Task<Message> {
            s.map_or_else(Task::none, |s| app.on_shortcut(s))
        }
        if let Some(menu) = self.menu.as_mut() {
            return match k.menu {
                Some(key) => {
                    let outcome = menu.key(key);
                    self.on_menu_outcome(outcome)
                }
                None => run(self, k.shortcut),
            };
        }
        if self.dialog.is_none()
            && let Some(viewer) = &self.code_dialog
        {
            use crate::features::code_view::Message as Code;
            let action = match k.shortcut {
                Some(Shortcut::FindInFile) => Some(Code::FindOpen),
                Some(Shortcut::Escape) if viewer.find_is_open() => Some(Code::FindClose),
                Some(Shortcut::Escape) => Some(Code::Close),
                Some(Shortcut::FindMatch { back }) if viewer.find_is_open() => {
                    Some(Code::FindMove { back })
                }
                Some(Shortcut::Copy) => Some(Code::Copy),
                Some(Shortcut::SelectAll) => Some(Code::SelectAll),
                _ => None,
            };
            if let Some(action) = action {
                return self.on_code_view(action);
            }
            if let Some(nav) = k.nav
                && nav.key == Key::Enter
                && viewer.find_is_open()
            {
                return self.on_code_view(Code::FindMove { back: nav.shift });
            }
            if self
                .pointer
                .field_at_key()
                .is(crate::features::code_view::find::FIELD_ID)
            {
                return Task::none();
            }
            return k.nav.map_or_else(Task::none, |nav| {
                viewer
                    .scroll_key(nav.key, &self.tokens)
                    .map(Message::CodeDialog)
            });
        }
        if self.settings_open && self.dialog.is_none() {
            match k.nav.map(|nav| nav.key) {
                Some(Key::Up) => self.cycle_theme(true),
                Some(Key::Down) => self.cycle_theme(false),
                _ => return run(self, k.shortcut),
            }
            return Task::none();
        }
        if k.shortcut == Some(Shortcut::Settings) {
            return self.on_shortcut(Shortcut::Settings);
        }
        if self.switcher.is_some() {
            return self.switcher_key(k);
        }
        if self.dialog.is_none() && self.picker.is_some() {
            return self.picker_key(k);
        }
        if self.dialog.is_none()
            && self
                .active_tab()
                .is_some_and(|tab| tab.history.file_search_is_open())
        {
            return self.history_file_search_key(k);
        }
        if self.dialog.is_none() && self.tab_list.is_some() {
            return self.tab_list_key(k);
        }
        if self.dialog.is_none() && self.undo_list.is_some() {
            return self.undo_list_key(k);
        }
        // Letter keys pick a dialog option (a dialog without text to type).
        if let (Some(dialog), Some(context_menu::Key::Char(c))) = (self.dialog.as_mut(), k.menu)
            && dialog.fields.is_empty()
            && dialog.area.is_none()
        {
            if c == 'l' && self.list_switch_blockers() {
                return Task::none();
            }
            if let Some(dialog) = self.dialog.as_mut().filter(|d| d.has_key(c)) {
                return match dialog.key(c) {
                    Some(outcome) => self.on_dialog_outcome(outcome),
                    None => Task::none(),
                };
            }
        }
        if let Some(dialog) = &self.dialog
            && let Some(nav) = k.nav
            && let Some(scroll) = dialog.review_nav(nav.key, &self.tokens)
        {
            return scroll.map(Message::Dialog);
        }
        if self.dialog.is_some() || self.rebase_editor().is_some() {
            // ⌘C / ⌘A belong to the dialog's text, not the diff behind it.
            return run(self, k.shortcut.filter(|s| !text_edit(*s)));
        }
        let field = self.pointer.field_at_key();
        if let Some(blame) = self
            .active_tab()
            .filter(|tab| tab.code_view == Role::Commit)
            .and_then(|tab| tab.blame.as_ref())
        {
            use crate::features::blame_view::{FIND_FIELD_ID, Message as Blame};
            let action = match k.shortcut {
                Some(Shortcut::FindInFile) => Some(Blame::FindOpen),
                Some(Shortcut::Escape)
                    if blame.find_is_open()
                        && (field == Field::None || field.is(FIND_FIELD_ID)) =>
                {
                    Some(Blame::FindClose)
                }
                Some(Shortcut::FindMatch { back }) if blame.find_is_open() => {
                    Some(Blame::FindMove { back })
                }
                _ => k
                    .nav
                    .filter(|nav| nav.key == Key::Enter && field.is(FIND_FIELD_ID))
                    .map(|nav| Blame::FindMove { back: nav.shift }),
            };
            if let Some(action) = action {
                return self.on_blame_message(action);
            }
        }
        if field != Field::None {
            let in_prompt = field.is(PROMPT_ID);
            // Esc leaves the field (the field itself drops focus).
            self.typing = in_prompt && k.shortcut != Some(Shortcut::Escape);
            if field.is(REFS_FILTER_ID)
                && let Some(n) = k.nav.filter(|n| {
                    matches!(
                        n.key,
                        Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Enter
                    )
                })
            {
                self.focus = Pane::Sidebar;
                return self.nav_pane(Pane::Sidebar, n).unwrap_or_else(Task::none);
            }
            // ⌘Z / ⌘⇧Z in a text field are about its text, not the repo.
            let s = k
                .shortcut
                .filter(|s| in_prompt || !prompt_only(*s))
                .filter(|s| !matches!(s, Shortcut::Undo | Shortcut::Redo))
                .filter(|s| !text_edit(*s));
            return run(self, s);
        }
        self.typing = false;
        if k.shortcut == Some(Shortcut::Escape)
            && self.focus == Pane::Diff
            && let Some(tab) = self.active_tab()
            && tab.diff(tab.code_view).has_selection()
        {
            return self.on_diff_message(tab.code_view, super::diff_view::Message::ClearSelection);
        }
        if let Some(task) = k.nav.and_then(|n| self.on_nav(n)) {
            return task;
        }
        run(self, k.shortcut.filter(|s| !prompt_only(*s)))
    }

    /// Keys while the ⌃⇥ switcher is up: ⇥ / ↑↓ move, ⏎ switches, Esc
    /// cancels (letting go of ⌃ switches too, see `Message::Modifiers`).
    fn switcher_key(&mut self, k: KeyInput) -> Task<Message> {
        match (k.shortcut, k.nav.map(|n| n.key)) {
            (Some(Shortcut::CycleTab { back }), _) => self.cycle_tab(back),
            (Some(Shortcut::Escape), _) => {
                self.switcher = None;
                Task::none()
            }
            (_, Some(Key::Up)) => self.cycle_tab(true),
            (_, Some(Key::Down)) => self.cycle_tab(false),
            (Some(Shortcut::Enter), _) | (_, Some(Key::Enter)) => self.finish_switch(),
            _ => Task::none(),
        }
    }

    /// Keys while the undo history is open: ↑/↓ move, ⏎ goes back to the
    /// highlighted step, Esc closes; ⌘Z / ⌘⇧Z still undo / redo one step.
    fn undo_list_key(&mut self, k: KeyInput) -> Task<Message> {
        use crate::features::undo_list;
        let rows = self
            .active_tab()
            .map(|t| undo_list::rows(&t.undo))
            .unwrap_or_default();
        let Some(list) = self.undo_list.as_mut() else {
            return Task::none();
        };
        if let Some(n) = k.nav {
            match n.key {
                Key::Up => list.move_by(&rows, -1),
                Key::Down => list.move_by(&rows, 1),
                Key::PageUp => list.move_by(&rows, -10),
                Key::PageDown => list.move_by(&rows, 10),
                Key::Enter => {
                    let row = list.current(&rows);
                    return self.update(Message::UndoList(undo_list::Message::Pick(row)));
                }
                _ => {}
            }
            return Task::none();
        }
        match k.shortcut {
            Some(Shortcut::Escape) => self.update(Message::UndoList(undo_list::Message::Dismiss)),
            Some(s @ (Shortcut::Undo | Shortcut::Redo)) => {
                self.undo_list = None;
                self.on_shortcut(s)
            }
            Some(s @ (Shortcut::Open | Shortcut::TabList | Shortcut::Quit)) => {
                self.undo_list = None;
                self.on_shortcut(s)
            }
            _ => Task::none(),
        }
    }

    /// Keys while the tab dropdown is open: the filter keeps typing;
    /// ↑/↓/PgUp/PgDn/⏎, ⌘W, ⌘1–9, ⌘P and Esc drive the list.
    fn tab_list_key(&mut self, k: KeyInput) -> Task<Message> {
        let keys: Vec<tab_list::Key> = self.tabs.iter().map(tab_list::Key::of).collect();
        let Some(list) = self.tab_list.as_mut() else {
            return Task::none();
        };
        if let Some(n) = k.nav {
            let by = match n.key {
                Key::Up => -1,
                Key::Down => 1,
                Key::PageUp => -tab_list::State::page(),
                Key::PageDown => tab_list::State::page(),
                Key::Enter => {
                    return match list.selected() {
                        Some(i) => self.update(Message::TabList(tab_list::Message::Pick(i))),
                        None => Task::none(),
                    };
                }
                // Everything else is typing in the filter.
                _ => return Task::none(),
            };
            list.move_by(by);
            return Task::none();
        }
        match k.shortcut {
            Some(Shortcut::Escape) => {
                if list.escape(&keys) {
                    self.close_tab_list()
                } else {
                    Task::none()
                }
            }
            Some(Shortcut::CloseTab) => match list.selected() {
                Some(i) => self.close_tab_from_list(i),
                None => Task::none(),
            },
            Some(Shortcut::SelectTab(i)) => {
                self.update(Message::TabList(tab_list::Message::Pick(i)))
            }
            Some(s @ (Shortcut::TabList | Shortcut::Open | Shortcut::Quit)) => self.on_shortcut(s),
            _ => Task::none(),
        }
    }

    fn history_file_search_key(&mut self, k: KeyInput) -> Task<Message> {
        let row_height = search::row_height(&self.tokens);
        if let Some(nav) = k.nav {
            return match nav.key {
                Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Home | Key::End => {
                    let Some(tab) = self.tabs.get_mut(self.active) else {
                        return Task::none();
                    };
                    let Some(state) = tab.history.search_state_mut() else {
                        return Task::none();
                    };
                    if state.move_to(nav.key, row_height) {
                        state.scroll_to_selected(row_height)
                    } else {
                        Task::none()
                    }
                }
                Key::Enter => {
                    let selected = self
                        .active_tab()
                        .and_then(|tab| tab.history.search_state())
                        .and_then(search::State::selected);
                    selected.map_or_else(Task::none, |row| {
                        self.update(Message::History(history::Message::Search(
                            search::Message::Activate(row),
                        )))
                    })
                }
                _ => Task::none(),
            };
        }
        match k.shortcut {
            Some(Shortcut::Escape) => self.update(Message::History(history::Message::Search(
                search::Message::Dismiss,
            ))),
            Some(
                s @ (Shortcut::CloseTab | Shortcut::TabList | Shortcut::Open | Shortcut::Quit),
            ) => {
                let dismiss = self.update(Message::History(history::Message::Search(
                    search::Message::Dismiss,
                )));
                Task::batch([dismiss, self.on_shortcut(s)])
            }
            _ => Task::none(),
        }
    }

    /// Keys while the open-repository window is up: the filter field keeps
    /// typing; ↑/↓/⏎, ⌘1–9, ⌘⌫, ⌘O and Esc drive the list.
    fn picker_key(&mut self, k: KeyInput) -> Task<Message> {
        let Some(picker) = self.picker.as_mut() else {
            return Task::none();
        };
        if let Some(n) = k.nav {
            let by = match n.key {
                Key::Up => -1,
                Key::Down => 1,
                Key::PageUp => -picker.page(),
                Key::PageDown => picker.page(),
                Key::Tab => {
                    return match picker.complete() {
                        Some(out) => self.on_picker_output(out),
                        None => Task::none(),
                    };
                }
                Key::Enter => {
                    return match picker.activate() {
                        Some(out) => self.on_picker_output(out),
                        None => Task::none(),
                    };
                }
                // Everything else is typing in the filter.
                _ => return Task::none(),
            };
            picker.move_by(by);
            return Task::none();
        }
        let out = match k.shortcut {
            Some(Shortcut::Escape) => picker.escape(),
            Some(Shortcut::Forget) => picker.forget_selected(),
            Some(Shortcut::Open) => Some(repo_picker::Output::Browse),
            Some(Shortcut::SelectTab(i)) => {
                return self.update(Message::Picker(repo_picker::Message::Pick(i)));
            }
            Some(Shortcut::Quit) => return iced::exit(),
            _ => None,
        };
        out.map_or_else(Task::none, |out| self.on_picker_output(out))
    }

    fn on_nav(&mut self, n: Nav) -> Option<Task<Message>> {
        if n.key == Key::Char('/') {
            return Some(self.focus_prompt());
        }
        self.active_tab()?;
        if n.key == Key::Tab {
            return Some(self.move_pane(if n.shift { -1 } else { 1 }, true));
        }
        let pane = self.pane();
        self.focus = pane;
        if let Some(task) = self.nav_pane(pane, n) {
            return Some(task);
        }
        match n.key {
            Key::Left => Some(self.move_pane(-1, false)),
            Key::Right | Key::Enter => Some(self.move_pane(1, false)),
            _ => None,
        }
    }

    fn move_pane(&mut self, by: isize, wrap: bool) -> Task<Message> {
        let panes = self.panes();
        let Some(i) = panes.iter().position(|p| *p == self.pane()) else {
            return Task::none();
        };
        #[allow(clippy::cast_possible_wrap)]
        let (i, n) = (i as isize, panes.len() as isize);
        let j = if wrap {
            (i + by).rem_euclid(n)
        } else {
            (i + by).clamp(0, n - 1)
        };
        #[allow(clippy::cast_sign_loss)]
        let next = panes[j as usize];
        self.focus = next;
        Task::none()
    }

    /// Hands a key to `pane`; `None` when the pane has no use for it.
    fn nav_pane(&mut self, pane: Pane, n: Nav) -> Option<Task<Message>> {
        let mode = self.refs_mode();
        let t = &self.tokens;
        let tab = self.tabs.get_mut(self.active)?;
        let snap = tab.snapshot.clone();
        let moved: Moved<Message> = match pane {
            Pane::Sidebar => match mode {
                crate::features::worktrees::RefsMode::Refs => tab
                    .sidebar
                    .nav(n, snap.as_deref(), t)?
                    .map(Message::Sidebar),
                crate::features::worktrees::RefsMode::Worktrees => {
                    tab.worktrees.nav(n, t)?.map(Message::Worktrees)
                }
            },
            Pane::History => tab
                .history
                .nav(n, snap.as_deref(), t)?
                .map(Message::History),
            Pane::Files => tab
                .commit_diff
                .files_nav(n, t, self.settings.appearance.history_file_dirs)?
                .map(Message::CommitDiff),
            Pane::Changes => tab
                .changes
                .nav(
                    n,
                    snap.as_deref()?,
                    t,
                    self.settings.appearance.working_tree_dirs,
                )?
                .map(Message::Changes),
            Pane::Diff => {
                let role = tab.code_view;
                let covered = match role {
                    Role::Commit => tab.blame.is_some(),
                    Role::Local => tab.conflict.is_some(),
                };
                if covered {
                    return None;
                }
                let wrap: fn(_) -> Message = match role {
                    Role::Local => Message::LocalDiff,
                    Role::Commit => Message::CommitDiff,
                };
                tab.diff_mut(role).diff_nav(n, t)?.map(wrap)
            }
        };
        // `.` opens the actions menu; from the keyboard it is centered.
        self.menu_by_key = n.key == Key::Char('.');
        let Moved { message, task } = moved;
        let follow = message.map_or_else(Task::none, |m| self.update(m));
        self.menu_by_key = false;
        Some(Task::batch([task, follow]))
    }
}

/// Frames `content` as `pane`: an accent border when it is the active one,
/// and a click anywhere in it makes it active.
pub(super) fn pane_frame<'a>(
    t: &Tokens,
    pane: Pane,
    active: Pane,
    content: Element<'a, Message>,
) -> Element<'a, Message> {
    let color = if pane == active {
        t.accent()
    } else {
        iced::Color::TRANSPARENT
    };
    mouse_area(
        container(content)
            .padding(1)
            .style(move |_| container::Style {
                border: Border {
                    color,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..container::Style::default()
            }),
    )
    .on_press(Message::FocusPane(pane))
    .into()
}
