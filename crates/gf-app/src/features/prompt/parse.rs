//! Parsing of what the user types at the `gf ❯` prompt.

use std::path::PathBuf;

use crate::widgets::format::expand_tilde;

/// A parsed prompt line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptCommand {
    /// A git command; the leading `git` is optional (`status` == `git status`).
    Git(Vec<String>),
    /// `:open [path]` — no path opens the folder picker.
    Open(Option<PathBuf>),
    CloseTab,
    SelectTab(usize),
    Refresh,
    /// `:theme [name]` — no name cycles through presets.
    Theme(Option<String>),
    ToggleLog,
    ClearLog,
    Help,
    Quit,
}

/// Built-in `:` commands with one-line descriptions, used by `:help`,
/// completion and the suggestion line.
pub const BUILTINS: &[(&str, &str)] = &[
    (":open", "open a repository (no path: folder picker)   ⌘O"),
    (":close", "close the current tab                         ⌘W"),
    (
        ":tab",
        ":tab N — switch to tab N                       ⌘1-9",
    ),
    (":refresh", "reload the current repository               ⌘R"),
    (":theme", ":theme [gf-dark|gf-light|dracula]"),
    (":log", "show/hide the command log                    ⌘J"),
    (":clear", "clear the command log"),
    (":help", "show this help                                ?"),
    (":quit", "quit gf                                       ⌘Q"),
];

/// Common git subcommands offered for completion. Any other subcommand still
/// runs; this list only drives suggestions.
pub const GIT_SUBCOMMANDS: &[&str] = &[
    "add",
    "bisect",
    "blame",
    "branch",
    "checkout",
    "cherry-pick",
    "clean",
    "clone",
    "commit",
    "config",
    "describe",
    "diff",
    "fetch",
    "gc",
    "grep",
    "init",
    "log",
    "merge",
    "mv",
    "notes",
    "pull",
    "push",
    "rebase",
    "reflog",
    "remote",
    "reset",
    "restore",
    "revert",
    "rm",
    "shortlog",
    "show",
    "stash",
    "status",
    "submodule",
    "switch",
    "tag",
    "worktree",
];

pub fn parse(line: &str) -> Result<Option<PromptCommand>, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    if line == "?" {
        return Ok(Some(PromptCommand::Help));
    }
    let mut words = split_words(line)?;

    if let Some(cmd) = words.first().and_then(|w| w.strip_prefix(':')) {
        let arg = words.get(1).cloned();
        let parsed = match cmd {
            "open" | "o" => PromptCommand::Open(arg.map(|a| expand_tilde(&a))),
            "close" | "c" => PromptCommand::CloseTab,
            "tab" | "t" => {
                let n = arg
                    .and_then(|a| a.parse::<usize>().ok())
                    .filter(|n| *n >= 1)
                    .ok_or_else(|| ":tab needs a tab number, e.g. :tab 2".to_owned())?;
                PromptCommand::SelectTab(n - 1)
            }
            "refresh" | "r" => PromptCommand::Refresh,
            "theme" => PromptCommand::Theme(arg),
            "log" => PromptCommand::ToggleLog,
            "clear" => PromptCommand::ClearLog,
            "help" | "h" => PromptCommand::Help,
            "quit" | "q" => PromptCommand::Quit,
            other => {
                return Err(format!(
                    "unknown command :{other} — type :help for the list"
                ));
            }
        };
        return Ok(Some(parsed));
    }

    if words.first().map(String::as_str) == Some("git") {
        words.remove(0);
    }
    if words.is_empty() {
        return Err("type a git subcommand, e.g. `status`".into());
    }
    Ok(Some(PromptCommand::Git(words)))
}

/// Shell-like word splitting: whitespace separates words, `'…'` is literal,
/// `"…"` allows `\"` and `\\` escapes, and `\` escapes the next character
/// outside quotes.
pub fn split_words(s: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err("unclosed ' quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(e @ ('"' | '\\')) => cur.push(e),
                            Some(other) => {
                                cur.push('\\');
                                cur.push(other);
                            }
                            None => return Err("unclosed \" quote".into()),
                        },
                        Some(c) => cur.push(c),
                        None => return Err("unclosed \" quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// Candidates for the word being typed (only the first word is completed).
pub fn candidates(input: &str) -> Vec<&'static str> {
    let trimmed = input.trim_start();
    let word = trimmed.strip_prefix("git ").unwrap_or(trimmed);
    if word.is_empty() || word.contains(char::is_whitespace) {
        return Vec::new();
    }
    if word.starts_with(':') {
        BUILTINS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| name.starts_with(word) && *name != word)
            .collect()
    } else {
        GIT_SUBCOMMANDS
            .iter()
            .copied()
            .filter(|name| name.starts_with(word) && *name != word)
            .collect()
    }
}

/// Tab completion: completes to the single candidate (plus a space) or to
/// the longest common prefix of several. `None` when nothing to add.
pub fn complete(input: &str) -> Option<String> {
    let found = candidates(input);
    let first = *found.first()?;
    let prefix_len = input.len() - input.trim_start().len();
    let git_prefix = if input.trim_start().starts_with("git ") {
        "git "
    } else {
        ""
    };
    let head = &input[..prefix_len];

    if found.len() == 1 {
        return Some(format!("{head}{git_prefix}{first} "));
    }
    let common = found.iter().fold(first.to_owned(), |acc, c| {
        acc.chars()
            .zip(c.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect()
    });
    let completed = format!("{head}{git_prefix}{common}");
    (completed.len() > input.len()).then_some(completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(args: &[&str]) -> Option<PromptCommand> {
        Some(PromptCommand::Git(
            args.iter().map(|s| (*s).to_owned()).collect(),
        ))
    }

    #[test]
    fn parses_git_commands_with_or_without_prefix() {
        assert_eq!(parse("status -sb"), Ok(git(&["status", "-sb"])));
        assert_eq!(parse("git status -sb"), Ok(git(&["status", "-sb"])));
        assert_eq!(
            parse(r#"commit -m "fix: it's \"done\"""#),
            Ok(git(&["commit", "-m", r#"fix: it's "done""#]))
        );
        assert_eq!(
            parse("log --format='%h %s'"),
            Ok(git(&["log", "--format=%h %s"]))
        );
        assert_eq!(parse("   "), Ok(None));
        assert!(parse("git").is_err());
    }

    #[test]
    fn parses_builtins() {
        assert_eq!(
            parse(":open /tmp/x"),
            Ok(Some(PromptCommand::Open(Some("/tmp/x".into()))))
        );
        assert_eq!(parse(":open"), Ok(Some(PromptCommand::Open(None))));
        assert_eq!(parse(":tab 2"), Ok(Some(PromptCommand::SelectTab(1))));
        assert!(parse(":tab 0").is_err());
        assert_eq!(
            parse(":theme dracula"),
            Ok(Some(PromptCommand::Theme(Some("dracula".into()))))
        );
        assert_eq!(parse("?"), Ok(Some(PromptCommand::Help)));
        assert_eq!(parse(":q"), Ok(Some(PromptCommand::Quit)));
        assert!(
            parse(":nope")
                .expect_err("unknown builtin")
                .contains(":help")
        );
    }

    #[test]
    fn splits_words() {
        assert_eq!(
            split_words(r"a\ b 'c d' e").ok(),
            Some(vec!["a b".into(), "c d".into(), "e".into()])
        );
        assert_eq!(split_words("''").ok(), Some(vec![String::new()]));
        assert!(split_words("'open").is_err());
        assert!(split_words("\"open").is_err());
    }

    #[test]
    fn completes_first_word() {
        assert_eq!(complete("sta"), None, "status and stash share only `sta`");
        assert_eq!(candidates("sta"), vec!["stash", "status"]);
        assert_eq!(complete("stat").as_deref(), Some("status "));
        assert_eq!(complete("git sw").as_deref(), Some("git switch "));
        assert_eq!(complete(":op").as_deref(), Some(":open "));
        assert_eq!(
            complete("st").as_deref(),
            Some("sta"),
            "common prefix of stash/status"
        );
        assert_eq!(complete("che"), None, "checkout/cherry-pick add nothing");
        assert_eq!(complete("status -"), None);
        assert!(candidates("").is_empty());
    }
}
