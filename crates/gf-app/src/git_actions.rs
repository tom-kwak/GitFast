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

/// What went wrong and how to get out of it, for the failure dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    pub title: &'static str,
    pub cause: &'static str,
    pub steps: &'static [&'static str],
}

struct Rule {
    /// Any of these (lowercase) in the error text selects the rule.
    needles: &'static [&'static str],
    diagnosis: Diagnosis,
}

const fn rule(
    needles: &'static [&'static str],
    title: &'static str,
    cause: &'static str,
    steps: &'static [&'static str],
) -> Rule {
    Rule {
        needles,
        diagnosis: Diagnosis {
            title,
            cause,
            steps,
        },
    }
}

/// Known failures, most specific first: the first match wins.
const RULES: &[Rule] = &[
    rule(
        &["not a git repository"],
        "Not a git repository",
        "The folder is not (or no longer) a git repository.",
        &[
            "Open a repository with ⌘O.",
            "Or create one with `init`, or `clone <url>`.",
        ],
    ),
    rule(
        &["index.lock", "another git process"],
        "Repository is locked",
        "Another git process is using the repository, or one crashed and left a lock file.",
        &[
            "Wait for other git tools (IDE, terminal) to finish, then retry.",
            "If nothing else is running, delete .git/index.lock and retry.",
        ],
    ),
    rule(
        &[
            "authentication failed",
            "could not read username",
            "could not read password",
            "permission denied (publickey",
            "invalid username or password",
            "terminal prompts disabled",
            "host key verification failed",
        ],
        "Authentication failed",
        "The remote refused your credentials (or none were available).",
        &[
            "SSH remote: add your key to ssh-agent (`ssh-add`) and to the host account.",
            "HTTPS remote: store a token with a credential helper (osxkeychain).",
            "Check access with `ssh -T git@<host>` in a terminal.",
        ],
    ),
    rule(
        &[
            "could not resolve host",
            "connection timed out",
            "connection refused",
            "network is unreachable",
            "operation timed out",
            "could not connect",
            "failed to connect",
            "ssl certificate",
        ],
        "Cannot reach the remote",
        "The network connection to the remote host failed.",
        &[
            "Check your network, VPN or proxy.",
            "Check the remote URL (`remote -v`).",
            "Retry once the connection is back.",
        ],
    ),
    rule(
        &[
            "repository not found",
            "does not appear to be a git repository",
            "no such remote",
        ],
        "Remote repository not found",
        "The remote URL is wrong, the repository moved, or you have no access to it.",
        &[
            "Check the remote URL (`remote -v`).",
            "Make sure your account can see the repository.",
        ],
    ),
    rule(
        &["protected branch", "pre-receive hook declined", "gh006"],
        "Push blocked by the server",
        "The branch is protected or a server hook refused the push.",
        &[
            "Push to a new branch and open a pull/merge request.",
            "Or ask a maintainer to allow pushes to this branch.",
        ],
    ),
    rule(
        &[
            "non-fast-forward",
            "fetch first",
            "updates were rejected",
            "[rejected]",
        ],
        "Push rejected",
        "The remote branch has commits you don't have yet.",
        &[
            "Pull (merge or rebase) to bring them in, then push again.",
            "Only if you mean to replace them: force push with lease.",
        ],
    ),
    rule(
        &[
            "has no upstream branch",
            "no tracking information",
            "no upstream configured",
        ],
        "No upstream branch",
        "The current branch is not linked to a remote branch.",
        &[
            "Publish the branch: push it with `push -u origin <branch>`.",
            "Or link it: `branch --set-upstream-to=origin/<branch>`.",
        ],
    ),
    rule(
        &["divergent branches", "need to specify how to reconcile"],
        "Branches have diverged",
        "Your branch and its upstream both have new commits.",
        &[
            "Pull again and choose merge or rebase.",
            "To set a default: `config pull.rebase false` (merge) or `true` (rebase).",
        ],
    ),
    rule(
        &[
            "would be overwritten by",
            "you have unstaged changes",
            "index contains uncommitted changes",
        ],
        "Local changes in the way",
        "Uncommitted changes would be overwritten by this operation.",
        &[
            "Commit the changes, or stash them (stash, run again, then pop).",
            "Or discard them in the working tree if you don't need them.",
        ],
    ),
    rule(
        &["untracked working tree files would be"],
        "Untracked files in the way",
        "Files that git does not track would be overwritten or removed.",
        &["Move, delete or commit those files, then retry."],
    ),
    rule(
        &[
            "conflict (",
            "automatic merge failed",
            "could not apply",
            "fix conflicts",
            "unmerged files",
            "you have unmerged paths",
        ],
        "Conflicts",
        "Both sides changed the same lines; git needs you to choose.",
        &[
            "Open WORKING TREE and resolve each conflicted file.",
            "Stage the resolved files, then continue the operation.",
            "Or abort to go back to where you started.",
        ],
    ),
    rule(
        &[
            "in the middle of",
            "already in progress",
            "not concluded your merge",
            "rebase-merge directory",
            "cherry-pick is already",
        ],
        "Another operation is in progress",
        "A merge, rebase, cherry-pick or revert was started and not finished.",
        &[
            "Finish it (resolve, stage, continue) from the banner,",
            "or abort it to go back to where you started.",
        ],
    ),
    rule(
        &["refusing to merge unrelated histories"],
        "Unrelated histories",
        "The two branches have no common commit.",
        &[
            "Check that you picked the right branch or remote.",
            "If intended, merge with `--allow-unrelated-histories` from the prompt.",
        ],
    ),
    rule(
        &[
            "please tell me who you are",
            "unable to auto-detect email",
            "empty ident name",
        ],
        "Who are you?",
        "git needs your name and email to record commits.",
        &[
            "Run `config --global user.name \"Your Name\"`.",
            "Run `config --global user.email you@example.com`.",
        ],
    ),
    rule(
        &["gpg failed to sign", "error: gpg", "signing failed"],
        "Signing failed",
        "Commit signing is turned on but the key could not be used.",
        &[
            "Unlock the key or check gpg-agent / your SSH signing key.",
            "Or turn signing off: `config commit.gpgsign false`.",
        ],
    ),
    rule(
        &["hook", "husky", "lint-staged"],
        "Rejected by a hook",
        "A repository hook (pre-commit, commit-msg, pre-push…) failed.",
        &[
            "Read the hook's output above and fix what it reports.",
            "Then stage the fixes and try again.",
        ],
    ),
    rule(
        &[
            "nothing to commit",
            "no changes added to commit",
            "no local changes to save",
        ],
        "Nothing to do",
        "There are no (staged) changes for this command.",
        &[
            "Stage files in WORKING TREE first.",
            "Use Amend to change only the last commit's message.",
        ],
    ),
    rule(
        &["not fully merged"],
        "Branch not merged",
        "Deleting it would lose commits that are on no other branch.",
        &[
            "Merge the branch first, or",
            "delete it with force (`branch -D <name>`) if you don't need them.",
        ],
    ),
    rule(
        &["is already checked out at", "is already used by worktree"],
        "Branch used by another worktree",
        "Another worktree has this branch checked out.",
        &["Switch that worktree to another branch, or remove the worktree."],
    ),
    rule(
        &["already exists"],
        "Already exists",
        "A branch, tag, remote or file with that name already exists.",
        &["Pick another name, or delete the existing one first."],
    ),
    rule(
        &["you are not currently on a branch"],
        "Not on a branch",
        "HEAD is detached, so there is no branch to update.",
        &[
            "Check out a branch first,",
            "or create one here to keep these commits.",
        ],
    ),
    rule(
        &["cannot lock ref", "unable to update local ref"],
        "Cannot update a ref",
        "A branch ref is locked or clashes with another name (e.g. `a` vs `a/b`).",
        &[
            "Run `remote prune origin` to drop stale remote branches.",
            "Retry; if a .lock file stays behind under .git/refs, delete it.",
        ],
    ),
    rule(
        &[
            "pathspec",
            "unknown revision",
            "invalid reference",
            "not a valid object name",
            "bad revision",
            "ambiguous argument",
            "couldn't find remote ref",
        ],
        "Not found",
        "A branch, commit or path named in the command does not exist.",
        &[
            "Refresh (the list may be stale) and check the name.",
            "For remote branches, fetch first.",
        ],
    ),
    rule(
        &[
            "exceeds github's file size limit",
            "file size limit",
            "large files detected",
        ],
        "File too large",
        "The remote refuses files above its size limit.",
        &[
            "Remove the file from the commits (amend / interactive rebase),",
            "or track it with Git LFS.",
        ],
    ),
    rule(
        &["no space left on device", "disk quota exceeded"],
        "Disk full",
        "There is no free space left to write to the repository.",
        &["Free some disk space, then retry."],
    ),
    rule(
        &["problem with the editor", "terminal is dumb"],
        "No editor available",
        "git tried to open an interactive editor, which gf cannot show.",
        &["Pass the message inline, e.g. `commit -m \"message\"`."],
    ),
    rule(
        &["is not a git command"],
        "Unknown command",
        "git does not know this subcommand.",
        &["Press ⇥ at the prompt to see suggestions."],
    ),
];

/// Used when no rule matches.
const UNKNOWN: Diagnosis = Diagnosis {
    title: "Git command failed",
    cause: "git stopped with an error (shown above).",
    steps: &[
        "Read git's message above — it usually names the problem.",
        "The full output is in the command log (⌘J).",
    ],
};

/// Classifies a git failure for the failure dialog.
pub fn diagnose(error: &str) -> Diagnosis {
    let lower = error.to_lowercase();
    RULES
        .iter()
        .find(|r| r.needles.iter().any(|n| lower.contains(n)))
        .map_or(UNKNOWN, |r| r.diagnosis.clone())
}

/// The part of a failure worth showing: git's own words without the
/// "`git …` failed with exit code" wrapper, at most `max` non-empty lines.
pub fn error_excerpt(error: &str, max: usize) -> Vec<String> {
    let body = match error.find("failed with exit code") {
        Some(at) => error[at..].split_once(": ").map_or(error, |(_, rest)| rest),
        None => error,
    };
    let mut lines: Vec<String> = body
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .map(|l| crate::widgets::format::truncate(l, 90))
        .collect();
    if lines.len() > max {
        let more = lines.len() - max;
        lines.truncate(max);
        lines.push(format!("… {more} more line(s) in the command log"));
    }
    if lines.is_empty() {
        lines.push(crate::widgets::format::truncate(error.trim(), 90));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnoses_common_failures() {
        let cases = [
            (
                "`git push` failed with exit code Some(1): ! [rejected] main -> main (fetch first)",
                "Push rejected",
            ),
            (
                "fatal: Authentication failed for 'https://x.org/a.git/'",
                "Authentication failed",
            ),
            (
                "git@x.org: Permission denied (publickey).",
                "Authentication failed",
            ),
            (
                "fatal: unable to access '…': Could not resolve host: x.org",
                "Cannot reach the remote",
            ),
            (
                "fatal: Unable to create '/r/.git/index.lock': File exists.",
                "Repository is locked",
            ),
            (
                "error: Your local changes to the following files would be overwritten by checkout:",
                "Local changes in the way",
            ),
            (
                "CONFLICT (content): Merge conflict in a.txt\nAutomatic merge failed",
                "Conflicts",
            ),
            (
                "fatal: The current branch dev has no upstream branch.",
                "No upstream branch",
            ),
            (
                "error: The branch 'x' is not fully merged.",
                "Branch not merged",
            ),
            ("fatal: a branch named 'x' already exists", "Already exists"),
            (
                "error: pathspec 'nope' did not match any file(s) known to git",
                "Not found",
            ),
            (
                "remote: error: GH006: Protected branch update failed",
                "Push blocked by the server",
            ),
            (
                "husky - pre-commit hook exited with code 1",
                "Rejected by a hook",
            ),
            (
                "Author identity unknown\n*** Please tell me who you are.",
                "Who are you?",
            ),
            ("something nobody has seen", "Git command failed"),
        ];
        for (error, title) in cases {
            assert_eq!(diagnose(error).title, title, "for {error:?}");
            assert!(!diagnose(error).steps.is_empty());
        }
    }

    #[test]
    fn excerpt_strips_the_wrapper_and_caps_lines() {
        let e = "`git push origin` failed with exit code Some(1): To x.org\n\n ! [rejected] main\nerror: failed";
        assert_eq!(
            error_excerpt(e, 5),
            ["To x.org", " ! [rejected] main", "error: failed"]
        );
        let many = (0..10)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = error_excerpt(&many, 3);
        assert_eq!(out.len(), 4);
        assert!(out[3].contains("7 more"));
        assert_eq!(error_excerpt("  ", 3).len(), 1);
    }

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
