//! Routes the settings view and prompt to the app's shared preferences.

use iced::Task;
use iced::advanced::widget::operation::focusable;

use super::{App, Message};
use crate::features::{command_log::NoteKind, settings_view};
use crate::theme;

impl App {
    pub(super) fn toggle_settings(&mut self) -> Task<Message> {
        if self.dialog.is_some()
            || self.picker.is_some()
            || self.switcher.is_some()
            || self.rebase_editor.is_some()
            || self.menu.is_some()
        {
            return Task::none();
        }
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.tab_list = None;
            self.undo_list = None;
            iced::advanced::widget::operate(focusable::unfocus())
        } else {
            self.restore_focus()
        }
    }

    pub(super) fn on_settings(&mut self, message: settings_view::Message) -> Task<Message> {
        if !self.settings_open || self.dialog.is_some() {
            return Task::none();
        }
        match message {
            settings_view::Message::SetTheme(name) => {
                self.set_theme(name);
                Task::none()
            }
            settings_view::Message::Close => self.toggle_settings(),
        }
    }

    pub(super) fn cycle_theme(&mut self, backwards: bool) {
        let presets = theme::PRESET_NAMES;
        let current = presets.iter().position(|name| *name == self.tokens.name);
        let step = if backwards { presets.len() - 1 } else { 1 };
        let next = current.map_or(0, |index| (index + step) % presets.len());
        self.set_theme(presets[next]);
    }

    pub(super) fn set_theme(&mut self, name: &str) {
        self.tokens = theme::preset(name).with_font_size(self.settings.appearance.font_size);
        self.settings.appearance.theme = name.to_owned();
        self.flash(NoteKind::Info, format!("theme: {name}"));
        self.save_settings();
    }
}
