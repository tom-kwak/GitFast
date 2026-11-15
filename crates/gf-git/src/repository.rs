use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use gf_core::model::{RepoHandle, RepoId};
use gf_core::{GfError, Result};

use crate::cli::{GitCommand, GitOutput, GitRunner, RunOptions};

/// An opened repository. Read operations are defined in `crate::ops`, one
/// file per area; anything without a dedicated method can be executed with
/// [`Repository::run`].
#[derive(Debug, Clone)]
pub struct Repository {
    handle: RepoHandle,
    runner: GitRunner,
    #[cfg(feature = "gix")]
    pub(crate) gix: Option<crate::gix_reader::GixReader>,
}

impl Repository {
    /// Opens the repository containing `path` (any directory inside the
    /// working tree, the working tree root, or a bare repository).
    pub async fn open(path: impl AsRef<Path>, runner: GitRunner) -> Result<Self> {
        let path = path.as_ref();
        let probe = GitCommand::new(["rev-parse", "--is-bare-repository", "--absolute-git-dir"])
            .current_dir(path)
            .read_only()
            .allow_failure();
        let out = runner.run(&probe).await?;
        if !out.success() {
            return Err(GfError::NotARepository(path.to_path_buf()));
        }
        let stdout = out.stdout_str();
        let mut lines = stdout.lines();
        let is_bare = lines.next() == Some("true");
        let git_dir = PathBuf::from(
            lines
                .next()
                .ok_or_else(|| GfError::parse("rev-parse", stdout.clone()))?,
        );

        let workdir = if is_bare {
            None
        } else {
            let top = runner
                .run(
                    &GitCommand::new(["rev-parse", "--show-toplevel"])
                        .current_dir(path)
                        .read_only(),
                )
                .await?;
            Some(PathBuf::from(top.stdout_str().trim_end()))
        };

        let handle = RepoHandle {
            id: RepoId::new(),
            workdir,
            git_dir,
        };
        Ok(Self {
            #[cfg(feature = "gix")]
            gix: crate::gix_reader::GixReader::open(&handle.git_dir),
            handle,
            runner,
        })
    }

    pub fn handle(&self) -> &RepoHandle {
        &self.handle
    }

    pub fn id(&self) -> RepoId {
        self.handle.id
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
            .repo(self.id())
    }

    /// Runs any git subcommand in this repository. This is the generic path
    /// behind the command palette and the "all git commands" support.
    pub async fn run<I, S>(&self, args: I) -> Result<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.runner.run(&self.command(args).echo_stdout()).await
    }

    pub async fn run_command(&self, cmd: &GitCommand, opts: RunOptions) -> Result<GitOutput> {
        self.runner.run_with(cmd, opts).await
    }

    /// Runs a read-only command and returns its stdout.
    pub(crate) async fn read(&self, cmd: GitCommand) -> Result<Vec<u8>> {
        Ok(self.runner.run(&cmd.read_only()).await?.stdout)
    }
}
