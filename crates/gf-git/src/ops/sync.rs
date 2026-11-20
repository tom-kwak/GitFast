//! Talking to remotes: fetch, pull, push. All accept [`RunOptions`] so the
//! UI can show progress and offer cancel.

use gf_core::{GfError, Result};

use crate::backend::{CliBackend, SyncOps};
use crate::cli::RunOptions;

pub const CONFIGURED_PUSH_REWRITES: &str = "push configuration can overwrite or delete remote refs";

impl CliBackend {
    /// Plain `git push` uses the configured refspecs for its destination.
    /// Guard any force or deletion rule even if it happens to be a no-op now.
    pub(crate) async fn guard_default_push(&self, branch: &str) -> Result<()> {
        let cmd = self
            .command([
                "config",
                "--null",
                "--get-regexp",
                r"^(remote\..*\.(push|mirror|url)|remote\.pushdefault|branch\..*\.(remote|pushremote))$",
            ])
            .read_only()
            .allow_failure();
        let output = self.runner().run(&cmd).await?;
        if !matches!(output.exit_code, Some(0 | 1)) {
            return Err(GfError::Other(output.stderr));
        }
        if default_push_rewrites(&output.stdout_str(), branch) {
            return Err(GfError::Unsupported(CONFIGURED_PUSH_REWRITES));
        }
        Ok(())
    }
}

fn default_push_rewrites(config: &str, branch: &str) -> bool {
    let entries = || {
        config.split_terminator('\0').map(|entry| {
            entry
                .split_once('\n')
                .map_or((entry, None), |(key, value)| (key, Some(value)))
        })
    };
    let get = |key: &str| {
        entries()
            .filter_map(|(k, v)| (k == key).then_some(v))
            .next_back()
            .flatten()
    };
    let remote = get(format!("branch.{branch}.pushremote").as_str())
        .or_else(|| get("remote.pushdefault"))
        .or_else(|| get(format!("branch.{branch}.remote").as_str()));
    let fallback = {
        let mut remotes =
            entries().filter_map(|(key, _)| key.strip_prefix("remote.")?.strip_suffix(".url"));
        remotes
            .next()
            .filter(|first| remotes.all(|rest| rest == *first))
            .unwrap_or("origin")
    };
    let remote = remote.unwrap_or(fallback);
    let push_key = format!("remote.{remote}.push");
    let mirror_key = format!("remote.{remote}.mirror");
    let mirror = |value: &str| {
        !matches!(
            value.to_ascii_lowercase().as_str(),
            "" | "false" | "no" | "off" | "0"
        )
    };
    let mirrored = entries()
        .filter_map(|(key, value)| (key == mirror_key).then_some(value))
        .next_back()
        .is_some_and(|value| value.is_none_or(mirror));
    mirrored
        || entries().any(|(key, value)| {
            key == push_key
                && value.is_some_and(|v| {
                    v.starts_with('+') || v.strip_prefix(':').is_some_and(|dest| !dest.is_empty())
                })
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PullMode {
    /// Use the repository's `pull.rebase`/`pull.ff` configuration.
    #[default]
    Config,
    /// Only fast-forward; fail instead of creating a merge.
    FastForwardOnly,
    /// Fast-forward, temporarily stashing tracked changes first.
    FastForwardWithAutostash,
    Merge,
    Rebase,
}

#[derive(Debug, Clone, Default)]
pub struct PushOptions {
    /// Remote name; `None` uses the branch's configured remote.
    pub remote: Option<String>,
    /// Branch (or refspec) to push; `None` pushes the current branch.
    pub branch: Option<String>,
    /// `-u`: remember the remote branch as upstream.
    pub set_upstream: bool,
    /// `--force-with-lease`: overwrite the remote branch only if it is
    /// still where we last saw it. Plain `--force` is never used.
    pub force_with_lease: bool,
    pub tags: bool,
}

impl SyncOps for CliBackend {
    async fn fetch(&self, remote: Option<&str>, opts: RunOptions) -> Result<()> {
        let mut cmd = self.command(["fetch", "--prune", "--progress"]);
        cmd = match remote {
            Some(r) => cmd.arg(r),
            None => cmd.arg("--all"),
        };
        self.exec_with(cmd, opts).await?;
        Ok(())
    }

    async fn pull(&self, mode: PullMode, opts: RunOptions) -> Result<()> {
        let mut cmd = self.command(["pull", "--progress"]);
        cmd = match mode {
            PullMode::Config => cmd,
            PullMode::FastForwardOnly => cmd.args(["--ff-only", "--no-rebase", "--no-autostash"]),
            PullMode::FastForwardWithAutostash => {
                cmd.args(["--ff-only", "--no-rebase", "--autostash"])
            }
            PullMode::Merge => cmd.args(["--no-rebase", "--ff"]),
            PullMode::Rebase => cmd.args(["--rebase", "--ff"]),
        };
        self.exec_with(cmd, opts).await?;
        Ok(())
    }

    async fn push(&self, push: &PushOptions, opts: RunOptions) -> Result<()> {
        let mut cmd = self.command(["push", "--progress"]);
        if push.set_upstream {
            cmd = cmd.arg("--set-upstream");
        }
        if push.force_with_lease {
            cmd = cmd.arg("--force-with-lease");
        }
        if push.tags {
            cmd = cmd.arg("--tags");
        }
        if let Some(remote) = &push.remote {
            cmd = cmd.args(["--end-of-options", remote]);
            if let Some(branch) = &push.branch {
                cmd = cmd.arg(branch);
            }
        }
        self.exec_with(cmd, opts).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_push_reviews_force_deletion_and_mirroring_on_its_destination() {
        let tracking = "branch.topic.remote\norigin\0remote.origin.url\nlocal\0";
        for (settings, risky) in [
            (
                "remote.origin.push\nrefs/heads/topic:refs/heads/topic\0",
                false,
            ),
            ("remote.origin.push\n:\0", false),
            ("remote.origin.push\n+refs/heads/*:refs/heads/*\0", true),
            ("remote.origin.push\n:refs/heads/obsolete\0", true),
            ("remote.origin.mirror\ntrue\0", true),
            ("remote.origin.mirror\0", true),
            (
                "remote.origin.mirror\ntrue\0remote.origin.mirror\nfalse\0",
                false,
            ),
            (
                "remote.origin.mirror\ntrue\0branch.topic.pushremote\nfork\0",
                false,
            ),
            (
                "remote.origin.mirror\ntrue\0remote.pushdefault\nfork\0",
                false,
            ),
        ] {
            assert_eq!(
                default_push_rewrites(&format!("{tracking}{settings}"), "topic"),
                risky,
                "{settings:?}"
            );
        }
    }
}
