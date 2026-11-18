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
    Settings,
    /// `:theme [name]` — no name cycles through presets.
    Theme(Option<String>),
    ToggleLog,
    ToggleRefs,
    ToggleLocal,
    ClearLog,
    Help,
    Quit,
    /// `pwd`: show the active repository's path.
    Pwd,
    /// `:undo`: undo the last step; `:undo list` shows the history.
    Undo {
        list: bool,
    },
    /// `:redo`: redo the last undone step.
    Redo,
    /// `:confirm` lists skipped confirmations; `:confirm reset` asks again.
    Confirms {
        reset: bool,
    },
    /// `:gix-fetch [on|off]`: fetch with gix instead of `git fetch`; no
    /// argument shows the current choice.
    GixFetch(Option<bool>),
}

/// Shell commands people type out of terminal habit that are not git
/// subcommands (`rm`/`mv` are, so they are not listed).
const SHELL_ONLY: &[&str] = &[
    "ls", "ll", "cat", "less", "echo", "mkdir", "touch", "cp", "vim", "vi", "nano", "code", "open",
    "npm", "yarn", "pnpm", "cargo", "make", "python", "node", "sudo", "brew", "chmod", "find",
    "head", "tail",
];

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
    (
        ":settings",
        "open app settings                           ⌘,",
    ),
    (":theme", ":theme [gf-dark|gf-light|dracula]"),
    (":log", "show/hide the command log                    ⌘J"),
    (":refs", "show/hide the refs sidebar                   ⌘B"),
    (
        ":confirm",
        ":confirm [reset] — confirmations you chose to skip",
    ),
    (
        ":undo",
        ":undo [list] — undo the last change made in gf (⌘Z), or list them",
    ),
    (":redo", "redo the last undone change                  ⌘⇧Z"),
    (":local", "open/close the working tree (else history)    ⌘E"),
    (
        ":gix-fetch",
        ":gix-fetch [on|off] — fetch in-process with gix (git as fallback)",
    ),
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
            "settings" => PromptCommand::Settings,
            "theme" => PromptCommand::Theme(arg),
            "log" => PromptCommand::ToggleLog,
            "refs" => PromptCommand::ToggleRefs,
            "undo" | "u" => match arg.as_deref() {
                None => PromptCommand::Undo { list: false },
                Some("list") => PromptCommand::Undo { list: true },
                Some(other) => {
                    return Err(format!("unknown option `{other}` — use `:undo list`"));
                }
            },
            "redo" => PromptCommand::Redo,
            "confirm" => match arg.as_deref() {
                None => PromptCommand::Confirms { reset: false },
                Some("reset") => PromptCommand::Confirms { reset: true },
                Some(other) => {
                    return Err(format!("unknown option `{other}` — use `:confirm reset`"));
                }
            },
            "gix-fetch" => PromptCommand::GixFetch(match arg.as_deref() {
                None => None,
                Some("on") => Some(true),
                Some("off") => Some(false),
                Some(other) => {
                    return Err(format!(
                        "unknown option `{other}` — use `:gix-fetch on` or `off`"
                    ));
                }
            }),
            "local" => PromptCommand::ToggleLocal,
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

    // Terminal habits: handle the few shell commands that map to gf
    // actions, and explain the rest instead of failing as `git ls`.
    match words.first().map(String::as_str) {
        Some("pwd") => return Ok(Some(PromptCommand::Pwd)),
        Some("clear" | "cls") => return Ok(Some(PromptCommand::ClearLog)),
        Some("exit") => return Ok(Some(PromptCommand::Quit)),
        Some("cd") => {
            return match words.get(1) {
                Some(dir) => Ok(Some(PromptCommand::Open(Some(expand_tilde(dir))))),
                None => Err("cd needs a folder — `cd ~/work/repo` opens that repository".into()),
            };
        }
        Some(cmd) if SHELL_ONLY.contains(&cmd) => {
            return Err(format!(
                "`{cmd}` is a shell command — this prompt runs git commands (a built-in terminal is planned)"
            ));
        }
        _ => {}
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
        assert_eq!(parse(":settings"), Ok(Some(PromptCommand::Settings)));
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

    #[test]
    fn handles_shell_habits() {
        assert_eq!(parse("pwd"), Ok(Some(PromptCommand::Pwd)));
        assert_eq!(parse("clear"), Ok(Some(PromptCommand::ClearLog)));
        assert_eq!(
            parse("cd /tmp/repo"),
            Ok(Some(PromptCommand::Open(Some("/tmp/repo".into()))))
        );
        assert!(parse("cd").is_err());
        assert!(
            parse("ls -la")
                .expect_err("shell only")
                .contains("shell command")
        );
        // git subcommands that share a name with shell commands still run.
        assert_eq!(parse("rm --cached a"), Ok(git(&["rm", "--cached", "a"])));
    }

    #[test]
    fn confirm_builtin() {
        assert_eq!(
            parse(":confirm"),
            Ok(Some(PromptCommand::Confirms { reset: false }))
        );
        assert_eq!(
            parse(":confirm reset"),
            Ok(Some(PromptCommand::Confirms { reset: true }))
        );
        assert!(parse(":confirm nope").is_err());
    }

    #[test]
    fn parses_gix_fetch_switch() {
        assert_eq!(parse(":gix-fetch"), Ok(Some(PromptCommand::GixFetch(None))));
        assert_eq!(
            parse(":gix-fetch on"),
            Ok(Some(PromptCommand::GixFetch(Some(true))))
        );
        assert_eq!(
            parse(":gix-fetch off"),
            Ok(Some(PromptCommand::GixFetch(Some(false))))
        );
        assert!(parse(":gix-fetch maybe").is_err());
    }
}
