//! [`CliBackend`]: runs the system `git` executable. It implements every
//! interface in [`crate::backend`]; the implementations live next to their
//! area's types in `crate::ops`. Hooks, credential helpers, LFS and signing
//! behave exactly as in the user's terminal.

use std::ffi::OsStr;
use std::path::Path;

use gf_core::Result;
use gf_core::model::RepoHandle;

use crate::cli::{GitCommand, GitOutput, GitRunner, RunOptions};

#[derive(Debug, Clone)]
pub struct CliBackend {
    handle: RepoHandle,
    runner: GitRunner,
}

impl CliBackend {
    pub fn new(handle: RepoHandle, runner: GitRunner) -> Self {
        Self { handle, runner }
    }

    pub fn handle(&self) -> &RepoHandle {
        &self.handle
    }

    pub fn runner(&self) -> &GitRunner {
        &self.runner
    }

    /// Directory git commands run in: the working tree, or the git dir for
    /// bare repositories.
    pub fn cwd(&self) -> &Path {
        self.handle
            .workdir
            .as_deref()
            .unwrap_or(&self.handle.git_dir)
    }

    /// A command bound to this repository (cwd + repo id for event tagging).
    pub fn command<I, S>(&self, args: I) -> GitCommand
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        GitCommand::new(args)
            .current_dir(self.cwd())
            .repo(self.handle.id)
    }

    /// Runs a command on behalf of the user: its output is shown in the
    /// command log. Fails on a non-zero exit code.
    pub(crate) async fn exec(&self, cmd: GitCommand) -> Result<GitOutput> {
        self.runner.run(&cmd.echo_stdout()).await
    }

    /// Like [`Self::exec`] with progress reporting and cancellation.
    pub(crate) async fn exec_with(&self, cmd: GitCommand, opts: RunOptions) -> Result<GitOutput> {
        self.runner.run_with(&cmd.echo_stdout(), opts).await
    }

    /// Runs a read-only command and returns its stdout.
    pub(crate) async fn read(&self, cmd: GitCommand) -> Result<Vec<u8>> {
        Ok(self.runner.run(&cmd.read_only()).await?.stdout)
    }
}
