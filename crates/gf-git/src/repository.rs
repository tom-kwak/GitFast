use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use gf_core::model::{RepoHandle, RepoId};
use gf_core::{GfError, Result};

use crate::backend::CliBackend;
use crate::cli::{GitCommand, GitOutput, GitRunner, RunOptions};

/// An opened repository: the UI's entry point. Each git command is a method
/// (see `routes.rs`) served by the backend chosen for it — the git CLI or
/// gix; anything without a dedicated method can be executed with
/// [`Repository::run`].
#[derive(Debug, Clone)]
pub struct Repository {
    pub(crate) cli: CliBackend,
    #[cfg(feature = "gix")]
    pub(crate) gix: Option<crate::backend::GixBackend>,
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
            gix: if cli_only() {
                None
            } else {
                crate::backend::GixBackend::open(&handle.git_dir, runner.clone(), handle.id)
            },
            cli: CliBackend::new(handle, runner),
        })
    }

    pub fn handle(&self) -> &RepoHandle {
        self.cli.handle()
    }

    /// The backends behind this repository, to compare their answers
    /// (contract tests). The gix one is `None` when `GF_GIT_BACKEND=cli`
    /// is set or gix could not open the repository.
    #[cfg(feature = "gix")]
    pub fn backends(&self) -> (&CliBackend, Option<&crate::backend::GixBackend>) {
        (&self.cli, self.gix.as_ref())
    }

    pub fn id(&self) -> RepoId {
        self.cli.handle().id
    }

    pub fn runner(&self) -> &GitRunner {
        self.cli.runner()
    }

    /// Directory git commands run in: the working tree, or the git dir for
    /// bare repositories.
    pub fn cwd(&self) -> &Path {
        self.cli.cwd()
    }

    /// A command bound to this repository (cwd + repo id for event tagging).
    pub fn command<I, S>(&self, args: I) -> GitCommand
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.cli.command(args)
    }

    /// Runs any git subcommand in this repository. This is the generic path
    /// behind the command palette and the "all git commands" support.
    pub async fn run<I, S>(&self, args: I) -> Result<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.runner().run(&self.command(args).echo_stdout()).await
    }

    pub async fn run_command(&self, cmd: &GitCommand, opts: RunOptions) -> Result<GitOutput> {
        self.runner().run_with(cmd, opts).await
    }
}

/// `GF_GIT_BACKEND=cli` turns gix off: every command uses the git CLI.
#[cfg(feature = "gix")]
fn cli_only() -> bool {
    std::env::var("GF_GIT_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("cli"))
}
