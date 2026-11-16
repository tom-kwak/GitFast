//! Bringing commits together: merge, rebase, cherry-pick, revert, and
//! continuing / skipping / aborting them when they stop on conflicts.
//!
//! Conflicts are an expected outcome, not an error: these return
//! [`Integration::Conflicts`] so the UI can show the resolution view.

use gf_core::{GfError, Result};

use super::state::RepoState;
use crate::backend::{CliBackend, IntegrateOps, StateOps};
use crate::cli::GitCommand;

/// Result of an integration step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Integration {
    Done,
    /// Stopped with unresolved conflicts in these many files.
    Conflicts(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeStyle {
    /// Fast-forward when possible, otherwise a merge commit.
    #[default]
    Default,
    /// Always create a merge commit.
    NoFastForward,
    /// Only fast-forward; fail otherwise.
    FastForwardOnly,
    /// Stage the combined changes without committing (`--squash`).
    Squash,
}

impl IntegrateOps for CliBackend {
    async fn conflicted_count(&self) -> Result<usize> {
        let out = self
            .read(self.command(["diff", "--name-only", "--diff-filter=U", "-z"]))
            .await?;
        Ok(out.split(|&b| b == 0).filter(|p| !p.is_empty()).count())
    }

    async fn merge(&self, rev: &str, style: MergeStyle) -> Result<Integration> {
        let mut cmd = self.command(["merge", "--no-edit"]);
        cmd = match style {
            MergeStyle::Default => cmd,
            MergeStyle::NoFastForward => cmd.arg("--no-ff"),
            MergeStyle::FastForwardOnly => cmd.arg("--ff-only"),
            MergeStyle::Squash => cmd.arg("--squash"),
        };
        self.integrate(cmd.args(["--end-of-options", rev])).await
    }

    async fn rebase(&self, onto: &str) -> Result<Integration> {
        self.integrate(self.command(["rebase", "--end-of-options", onto]))
            .await
    }

    async fn cherry_pick(&self, revs: &[String]) -> Result<Integration> {
        self.integrate(self.command(["cherry-pick", "--end-of-options"]).args(revs))
            .await
    }

    async fn revert(&self, rev: &str) -> Result<Integration> {
        self.revert_many(&[rev.to_owned()]).await
    }

    async fn revert_many(&self, revs: &[String]) -> Result<Integration> {
        self.integrate(
            self.command(["revert", "--no-edit", "--end-of-options"])
                .args(revs),
        )
        .await
    }

    async fn continue_operation(&self) -> Result<Integration> {
        let state = self.state().await?;
        let cmd = match Self::sequencer(&state)? {
            // A merge is concluded by committing the prepared merge.
            "merge" => self.command(["commit", "--no-edit"]),
            op => self.command([op, "--continue"]),
        };
        self.integrate(cmd).await
    }

    async fn skip_operation(&self) -> Result<Integration> {
        let state = self.state().await?;
        if !state.can_skip() {
            return Err(GfError::Other(
                "only a rebase, cherry-pick or revert can skip a commit".into(),
            ));
        }
        let op = Self::sequencer(&state)?;
        self.integrate(self.command([op, "--skip"])).await
    }

    async fn abort_operation(&self) -> Result<()> {
        let state = self.state().await?;
        let op = Self::sequencer(&state)?;
        self.exec(self.command([op, "--abort"])).await?;
        Ok(())
    }
}

impl CliBackend {
    /// Runs an integrating command; a non-zero exit that left conflicts
    /// behind becomes [`Integration::Conflicts`].
    async fn integrate(&self, cmd: GitCommand) -> Result<Integration> {
        // Steps that create commits must not wait for an editor: keep the
        // prepared message (`true` exits 0 without changing the file).
        let cmd = cmd.env("GIT_EDITOR", "true").allow_failure();
        let out = self.exec(cmd.clone()).await?;
        if out.success() {
            return Ok(Integration::Done);
        }
        let conflicts = self.conflicted_count().await?;
        if conflicts > 0 {
            return Ok(Integration::Conflicts(conflicts));
        }
        Err(GfError::GitCommand {
            argv: cmd
                .user_args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
            exit_code: out.exit_code,
            stderr: out.stderr.trim().to_owned(),
        })
    }

    /// For other modules (interactive rebase) that build their own command.
    pub(crate) async fn integrate_public(&self, cmd: GitCommand) -> Result<Integration> {
        self.integrate(cmd).await
    }

    fn sequencer(state: &RepoState) -> Result<&'static str> {
        Ok(match state {
            RepoState::Merging { .. } => "merge",
            RepoState::Rebasing { .. } => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::Bisecting | RepoState::Clean => {
                return Err(GfError::Other(
                    "no merge, rebase, cherry-pick or revert in progress".into(),
                ));
            }
        })
    }
}
