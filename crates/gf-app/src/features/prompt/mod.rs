//! The `gf ❯` command line: runs any git command or a `:` built-in, with
//! history (↑/↓), Tab completion and a live suggestion line. Commands typed
//! with the Korean input method active are recognized and converted.

pub mod hangul;
pub mod parse;

use iced::widget::{column, row, text_input};
use iced::{Element, Length, Padding};

use self::parse::{BUILTINS, GIT_SUBCOMMANDS, PromptCommand, candidates};
use crate::feature::{Cx, Feature, Update};
use crate::theme::style;
use crate::widgets::{colored, dim};

/// Widget id used to focus the prompt (⌘L, after closing dialogs, ...).
pub const PROMPT_ID: &str = "gf-prompt";

const MAX_HISTORY: usize = 200;

#[derive(Debug, Default)]
pub struct State {
    input: String,
    history: Vec<String>,
    /// Position while browsing history with ↑/↓; `None` = editing new line.
    cursor: Option<usize>,
    /// Draft kept while browsing history so ↓ past the end restores it.
    draft: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Input(String),
    Submit,
    HistoryPrev,
    HistoryNext,
    Complete,
}

#[derive(Debug, Clone)]
pub enum Output {
    Run {
        cmd: PromptCommand,
        /// Original text when the line was converted from Korean input.
        converted_from: Option<String>,
    },
    /// The line could not be parsed; shown in the command log.
    Invalid { line: String, error: String },
}

/// Whether `word` is something the prompt understands as a command name.
fn is_known_command(word: &str) -> bool {
    word == "git"
        || GIT_SUBCOMMANDS.contains(&word)
        || BUILTINS.iter().any(|(name, _)| *name == word)
}

impl State {
    fn remember(&mut self, line: &str) {
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_owned());
            if self.history.len() > MAX_HISTORY {
                self.history.remove(0);
            }
        }
    }
}

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message {
            Message::Input(s) => {
                self.input = s;
                self.cursor = None;
                Update::none()
            }
            Message::Submit => {
                let line = std::mem::take(&mut self.input);
                self.cursor = None;
                if line.trim().is_empty() {
                    return Update::none();
                }
                // `ㅣㅐㅎ -3` typed in Korean mode means `log -3`.
                let recovered = hangul::recover(&line, is_known_command);
                let (line, converted_from) = match recovered {
                    Some(converted) => (converted, Some(line)),
                    None => (line, None),
                };
                self.remember(line.trim());
                match parse::parse(&line) {
                    Ok(Some(cmd)) => Update::output(Output::Run {
                        cmd,
                        converted_from,
                    }),
                    Ok(None) => Update::none(),
                    Err(error) => Update::output(Output::Invalid { line, error }),
                }
            }
            Message::HistoryPrev => {
                if self.history.is_empty() {
                    return Update::none();
                }
                let next = match self.cursor {
                    None => {
                        self.draft = self.input.clone();
                        self.history.len() - 1
                    }
                    Some(i) => i.saturating_sub(1),
                };
                self.cursor = Some(next);
                self.input = self.history[next].clone();
                Update::task(iced::widget::operation::move_cursor_to_end(PROMPT_ID))
            }
            Message::HistoryNext => {
                match self.cursor {
                    Some(i) if i + 1 < self.history.len() => {
                        self.cursor = Some(i + 1);
                        self.input = self.history[i + 1].clone();
                    }
                    Some(_) => {
                        self.cursor = None;
                        self.input = std::mem::take(&mut self.draft);
                    }
                    None => {}
                }
                Update::task(iced::widget::operation::move_cursor_to_end(PROMPT_ID))
            }
            Message::Complete => {
                if let Some(completed) = parse::complete(&self.input) {
                    self.input = completed;
                }
                Update::task(iced::widget::operation::move_cursor_to_end(PROMPT_ID))
            }
        }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;
        // A placeholder is not drawn through `widgets::mono`: convert here.
        let placeholder = crate::platform::keys::local(
            "/ to type a git command (status, log -5 …)  ·  :help  ·  ⌘O open repo".into(),
        );
        let input = text_input(&placeholder, &self.input)
            .id(PROMPT_ID)
            .on_input(Message::Input)
            .on_submit(Message::Submit)
            .size(t.font_size)
            .padding(Padding::from([4, 0]))
            .style(style::prompt_input(t));

        let prompt = row![colored(t, "gf ❯", t.accent()), input]
            .spacing(8)
            .align_y(iced::Alignment::Center);

        // Suggestion line: Korean-input recovery, what Tab would complete, or
        // what a built-in does.
        let found = candidates(&self.input);
        let recovered = hangul::recover(&self.input, is_known_command);
        let hint = if let Some(converted) = recovered {
            Some(format!("한/영 ⏎ runs: {converted}"))
        } else if !found.is_empty() {
            Some(format!("tab ⇥  {}", found.join("  ")))
        } else {
            let first = self.input.split_whitespace().next().unwrap_or_default();
            BUILTINS
                .iter()
                .find(|(name, _)| *name == first)
                .map(|(_, help)| format!("{first} — {help}"))
        };

        // The hint line is always present (empty when there is nothing to
        // say): adding/removing it would shift the input's position in the
        // widget tree, which makes iced drop its focus mid-typing.
        column![dim(t, hint.unwrap_or_default()), prompt]
            .width(Length::Fill)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submit(state: &mut State, line: &str) -> Option<Output> {
        let _ = state.update(Message::Input(line.into()));
        state.update(Message::Submit).output
    }

    #[test]
    fn submit_parses_and_clears() {
        let mut s = State::default();
        let out = submit(&mut s, "status");
        assert!(matches!(
            out,
            Some(Output::Run {
                cmd: PromptCommand::Git(_),
                converted_from: None
            })
        ));
        assert!(s.input.is_empty());
        assert!(matches!(
            submit(&mut s, ":bogus"),
            Some(Output::Invalid { .. })
        ));
    }

    #[test]
    fn history_navigation_restores_draft() {
        let mut s = State::default();
        submit(&mut s, "status");
        submit(&mut s, "log -3");
        submit(&mut s, "log -3"); // consecutive duplicate not stored twice
        let _ = s.update(Message::Input("dra".into()));

        let _ = s.update(Message::HistoryPrev);
        assert_eq!(s.input, "log -3");
        let _ = s.update(Message::HistoryPrev);
        assert_eq!(s.input, "status");
        let _ = s.update(Message::HistoryPrev);
        assert_eq!(s.input, "status", "stays at the oldest entry");
        let _ = s.update(Message::HistoryNext);
        assert_eq!(s.input, "log -3");
        let _ = s.update(Message::HistoryNext);
        assert_eq!(s.input, "dra", "draft restored past the newest entry");
    }

    #[test]
    fn tab_completes() {
        let mut s = State::default();
        let _ = s.update(Message::Input("swi".into()));
        let _ = s.update(Message::Complete);
        assert_eq!(s.input, "switch ");
    }

    #[test]
    fn korean_input_is_recovered() {
        let mut s = State::default();
        let out = submit(&mut s, "ㅣㅐㅎ -3");
        match out {
            Some(Output::Run {
                cmd: PromptCommand::Git(args),
                converted_from: Some(original),
            }) => {
                assert_eq!(args, ["log", "-3"]);
                assert_eq!(original, "ㅣㅐㅎ -3");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(s.history.last().map(String::as_str), Some("log -3"));
    }
}
