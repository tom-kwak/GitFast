use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

use gf_core::model::RepoId;

/// Description of one git invocation. Built by ops or, for arbitrary
/// subcommands, directly from user input; executed by
/// [`GitRunner`](super::GitRunner).
#[derive(Clone, Default)]
pub struct GitCommand {
    pub(crate) args: Vec<OsString>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) env: Vec<(OsString, OsString)>,
    pub(crate) stdin: Option<Vec<u8>>,
    pub(crate) repo: Option<RepoId>,
    pub(crate) read_only: bool,
    pub(crate) allow_failure: bool,
    pub(crate) echo_stdout: bool,
}

impl fmt::Debug for GitCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Credential commands can carry secrets in their environment and stdin.
        f.debug_struct("GitCommand")
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

impl GitCommand {
    /// `args` are what follows `git`, e.g. `["status", "--short"]`.
    pub fn new<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self {
            args: args.into_iter().map(|a| a.as_ref().to_owned()).collect(),
            ..Self::default()
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    /// Appends `--` and the given paths (pathspecs).
    pub fn paths<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut paths = paths.into_iter().peekable();
        if paths.peek().is_some() {
            self.args.push("--".into());
            self.args
                .extend(paths.map(|p| p.as_ref().as_os_str().to_owned()));
        }
        self
    }

    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    pub fn stdin(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(data.into());
        self
    }

    /// Tags command events with the repository they belong to.
    pub fn repo(mut self, repo: RepoId) -> Self {
        self.repo = Some(repo);
        self
    }

    /// The command only reads. Adds `--no-optional-locks` so background
    /// refreshes never contend with the user's own git processes for
    /// `index.lock`.
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Return the output instead of an error when git exits non-zero
    /// (e.g. `merge` with conflicts, `diff --exit-code`).
    pub fn allow_failure(mut self) -> Self {
        self.allow_failure = true;
        self
    }

    /// Also forward stdout lines to the command log. Off by default because
    /// read commands can produce megabytes of machine-readable output.
    pub fn echo_stdout(mut self) -> Self {
        self.echo_stdout = true;
        self
    }

    /// The subcommand arguments as given (without gf's internal flags).
    pub fn user_args(&self) -> &[OsString] {
        &self.args
    }

    /// Human-readable form for logs and error messages: `git status -z`.
    pub fn display(&self) -> String {
        std::iter::once("git".to_owned())
            .chain(self.args.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_args_and_pathspecs() {
        let cmd = GitCommand::new(["diff"])
            .arg("--cached")
            .paths(["a.rs", "dir/b.rs"]);
        assert_eq!(cmd.display(), "git diff --cached -- a.rs dir/b.rs");
    }

    #[test]
    fn empty_pathspec_adds_no_separator() {
        let cmd = GitCommand::new(["status"]).paths(Vec::<PathBuf>::new());
        assert_eq!(cmd.display(), "git status");
    }

    #[test]
    fn debug_hides_process_environment_and_stdin() {
        let cmd = GitCommand::new(["credential", "approve"])
            .env("GF_LOGIN_TOKEN", "environment-secret")
            .stdin("password=stdin-secret");
        let debug = format!("{cmd:?}");
        assert!(debug.contains("credential"));
        assert!(!debug.contains("secret"));
    }
}
