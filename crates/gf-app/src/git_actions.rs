//! Helpers for running user-typed git commands from the prompt: where to
//! run them, what to open afterwards, and friendlier error hints.

use std::path::{Path, PathBuf};

/// Commands that create a repository rather than act on the current one.
pub fn creates_repository(args: &[String]) -> bool {
    matches!(args.first().map(String::as_str), Some("clone" | "init"))
}

/// The directory a successful `clone`/`init` produced, so gf can open it.
/// `cwd` is where the command ran.
pub fn created_repository(args: &[String], cwd: &Path) -> Option<PathBuf> {
    let (cmd, rest) = args.split_first()?;
    // Positional arguments, skipping options and the values of options that take one.
    let mut positional = Vec::new();
    let mut skip_next = false;
    for a in rest {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--" {
            continue;
        }
        if a.starts_with('-') {
            let takes_value = matches!(
                a.as_str(),
                "-b" | "--branch"
                    | "-o"
                    | "--origin"
                    | "--depth"
                    | "-c"
                    | "--config"
                    | "--template"
                    | "--separate-git-dir"
                    | "--reference"
                    | "-u"
                    | "--upload-pack"
                    | "-j"
                    | "--jobs"
                    | "--filter"
                    | "--initial-branch"
                    | "--object-format"
            );
            skip_next = takes_value;
            continue;
        }
        positional.push(a.as_str());
    }
    match cmd.as_str() {
        "clone" => {
            let url = positional.first()?;
            let dir = match positional.get(1) {
                Some(dir) => (*dir).to_owned(),
                None => humanish_name(url)?,
            };
            Some(cwd.join(dir))
        }
        "init" => Some(match positional.first() {
            Some(dir) => cwd.join(dir),
            None => cwd.to_path_buf(),
        }),
        _ => None,
    }
}

/// Directory name git derives from a clone URL: last path component without
/// a trailing `.git` (`git@host:team/app.git` → `app`).
fn humanish_name(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/').trim_end_matches("/.git");
    let last = trimmed.rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty()).then(|| name.to_owned())
}

/// A next-step suggestion for common failures, shown under the error.
pub fn hint_for_error(args: &[String], error: &str) -> Option<&'static str> {
    if error.contains("problem with the editor") || error.contains("Terminal is dumb") {
        return Some(
            "gf can't open an interactive editor yet — pass the message inline, e.g. commit -m \"message\"",
        );
    }
    if error.contains("not a git repository") {
        return Some("open a repository first (⌘O), or create one with `init` / `clone <url>`");
    }
    if error.contains("Authentication failed") || error.contains("could not read Username") {
        return Some(
            "authentication failed — set up an SSH key or a credential helper; account login arrives with the hosting integration",
        );
    }
    if error.contains("non-fast-forward") || error.contains("fetch first") {
        return Some(
            "the remote has commits you don't have — run `pull` (or `fetch` then merge/rebase) before pushing",
        );
    }
    if args.first().map(String::as_str) == Some("commit") && error.contains("nothing to commit") {
        return Some("nothing is staged — stage files with `add <path>` first");
    }
    if error.contains("is not a git command") {
        return Some("unknown git subcommand — press ⇥ at the prompt to see suggestions");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn finds_clone_target() {
        let cwd = Path::new("/work");
        assert_eq!(
            created_repository(&args(&["clone", "git@gitlab.com:team/app.git"]), cwd),
            Some("/work/app".into())
        );
        assert_eq!(
            created_repository(
                &args(&["clone", "--depth", "1", "https://x.org/a/b/", "dest"]),
                cwd
            ),
            Some("/work/dest".into())
        );
        assert_eq!(
            created_repository(
                &args(&["clone", "-b", "dev", "https://x.org/repo.git"]),
                cwd
            ),
            Some("/work/repo".into())
        );
        assert_eq!(
            created_repository(&args(&["init"]), cwd),
            Some("/work".into())
        );
        assert_eq!(
            created_repository(&args(&["init", "-b", "main", "new"]), cwd),
            Some("/work/new".into())
        );
        assert_eq!(created_repository(&args(&["status"]), cwd), None);
    }

    #[test]
    fn hints() {
        assert!(
            hint_for_error(
                &args(&["commit"]),
                "error: There was a problem with the editor 'false'"
            )
            .is_some()
        );
        assert!(
            hint_for_error(&args(&["push"]), "! [rejected] main -> main (fetch first)").is_some()
        );
        assert!(hint_for_error(&args(&["status"]), "all good").is_none());
    }
}
