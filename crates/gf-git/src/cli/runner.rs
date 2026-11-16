use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gf_core::event::CommandId;
use gf_core::model::RepoId;
use gf_core::{CommandEvent, CommandPhase, CoreEvent, EventBus, GfError, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{GitCommand, Progress};

/// Flags added to every invocation so output is stable and parseable no
/// matter what the user's git config says.
const BASE_ARGS: &[&str] = &[
    "--no-pager",
    "-c",
    "core.quotepath=false",
    "-c",
    "color.ui=false",
];

/// Captured result of a finished git process.
#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
    /// `None` if the process was terminated by a signal.
    pub exit_code: Option<i32>,
}

impl GitOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Per-call controls for a long-running command.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Kills the git process when cancelled; the call returns
    /// [`GfError::Cancelled`].
    pub cancel: Option<CancellationToken>,
    /// Receives parsed progress lines. Commands only report progress when
    /// given `--progress` (git suppresses it when stderr is not a TTY).
    pub progress: Option<mpsc::UnboundedSender<Progress>>,
}

/// Spawns git processes. Cheap to clone; clones share the command id counter
/// and event bus.
#[derive(Debug, Clone)]
pub struct GitRunner {
    git: PathBuf,
    env: Vec<(OsString, OsString)>,
    clear_env: bool,
    bus: Option<EventBus>,
    next_id: Arc<AtomicU64>,
}

impl Default for GitRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl GitRunner {
    /// Uses `git` from `PATH`.
    pub fn new() -> Self {
        Self {
            git: PathBuf::from("git"),
            env: Vec::new(),
            clear_env: false,
            bus: None,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Uses a specific git executable (user setting, bundled git, ...).
    pub fn with_git_path(mut self, git: impl Into<PathBuf>) -> Self {
        self.git = git.into();
        self
    }

    /// Publishes started/output/finished events for every command.
    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Environment variables added to every command.
    pub fn with_env<K, V>(mut self, env: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<OsString>,
        V: Into<OsString>,
    {
        self.env
            .extend(env.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Start every command from an empty environment plus [`Self::with_env`]
    /// variables. Used by tests to isolate from the developer's git config.
    pub fn with_cleared_env(mut self) -> Self {
        self.clear_env = true;
        self
    }

    /// The git executable this runner starts.
    pub fn git_executable(&self) -> &std::path::Path {
        &self.git
    }

    pub async fn run(&self, cmd: &GitCommand) -> Result<GitOutput> {
        self.run_with(cmd, RunOptions::default()).await
    }

    pub async fn run_with(&self, cmd: &GitCommand, opts: RunOptions) -> Result<GitOutput> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        self.emit(
            id,
            cmd,
            CommandPhase::Started {
                argv: std::iter::once("git".to_owned())
                    .chain(cmd.args.iter().map(|a| a.to_string_lossy().into_owned()))
                    .collect(),
                read_only: cmd.read_only,
            },
        );
        tracing::debug!(id, cmd = %cmd.display(), "git start");

        let result = self.execute(id, cmd, opts).await;

        let exit_code = result.as_ref().ok().and_then(|o| o.exit_code);
        self.emit(
            id,
            cmd,
            CommandPhase::Finished {
                exit_code,
                duration: started.elapsed(),
            },
        );
        tracing::debug!(id, ?exit_code, elapsed = ?started.elapsed(), "git end");

        let output = result?;
        if output.success() || cmd.allow_failure {
            Ok(output)
        } else {
            Err(GfError::GitCommand {
                argv: cmd
                    .args
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect(),
                exit_code: output.exit_code,
                stderr: output.stderr.trim().to_owned(),
            })
        }
    }

    async fn execute(
        &self,
        id: CommandId,
        cmd: &GitCommand,
        opts: RunOptions,
    ) -> Result<GitOutput> {
        let mut process = Command::new(&self.git);
        if self.clear_env {
            process.env_clear();
        }
        process
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .envs(cmd.env.iter().map(|(k, v)| (k, v)))
            // Never block on an interactive prompt; auth goes through askpass.
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(BASE_ARGS)
            .args(cmd.read_only.then_some("--no-optional-locks"))
            .args(&cmd.args)
            .stdin(if cmd.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &cmd.cwd {
            process.current_dir(cwd);
        }

        let mut child = process.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                GfError::Other(format!("git executable not found: {}", self.git.display()))
            }
            _ => GfError::Io(e),
        })?;

        if let (Some(data), Some(mut stdin)) = (cmd.stdin.clone(), child.stdin.take()) {
            tokio::spawn(async move {
                // A write error means git exited early; its exit code reports why.
                let _ = stdin.write_all(&data).await;
            });
        }

        let stdout_task = {
            let mut pipe = child
                .stdout
                .take()
                .ok_or_else(|| GfError::Other("no stdout".into()))?;
            let echo = cmd.echo_stdout.then(|| self.line_sink(id, cmd, false));
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 16 * 1024];
                let mut lines = LineSplitter::default();
                loop {
                    let n = pipe.read(&mut chunk).await?;
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(sink) = &echo {
                        lines.feed(&chunk[..n], sink);
                    }
                }
                if let Some(sink) = &echo {
                    lines.finish(sink);
                }
                Ok::<_, std::io::Error>(buf)
            })
        };

        let stderr_task = {
            let mut pipe = child
                .stderr
                .take()
                .ok_or_else(|| GfError::Other("no stderr".into()))?;
            let sink = self.line_sink(id, cmd, true);
            let progress = opts.progress;
            tokio::spawn(async move {
                let mut text = String::new();
                let mut chunk = [0u8; 4096];
                let mut lines = LineSplitter::default();
                // Progress redraws go to the progress channel only, so the
                // command log and error text are not flooded with them.
                let mut handle = |line: String| match Progress::parse(&line) {
                    Some(p) => {
                        if let Some(tx) = &progress {
                            let _ = tx.send(p);
                        }
                    }
                    None => {
                        text.push_str(&line);
                        text.push('\n');
                        sink(line);
                    }
                };
                loop {
                    let n = pipe.read(&mut chunk).await?;
                    if n == 0 {
                        break;
                    }
                    lines.feed(&chunk[..n], &mut handle);
                }
                lines.finish(&mut handle);
                Ok::<_, std::io::Error>(text)
            })
        };

        let cancel = opts.cancel.unwrap_or_default();
        let status = tokio::select! {
            status = child.wait() => status?,
            () = cancel.cancelled() => {
                let _ = child.kill().await;
                // Grandchildren (hooks, aliases) may still hold the pipes open;
                // stop reading instead of waiting for them.
                stdout_task.abort();
                stderr_task.abort();
                return Err(GfError::Cancelled);
            }
        };

        let stdout = join(stdout_task).await?;
        let stderr = join(stderr_task).await?;
        Ok(GitOutput {
            stdout,
            stderr,
            exit_code: status.code(),
        })
    }

    /// Reports an operation done in-process (by gix) like a command, so the
    /// command log lists every change made on the user's behalf. `argv`
    /// starts with the tool, e.g. `["gix", "branch", "x"]`.
    pub fn report_in_process(
        &self,
        repo: Option<RepoId>,
        argv: Vec<String>,
        lines: &[String],
        exit_code: i32,
        duration: Duration,
    ) {
        let Some(bus) = &self.bus else {
            return;
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let send = |phase| bus.publish(CoreEvent::Command(CommandEvent { id, repo, phase }));
        send(CommandPhase::Started {
            argv,
            read_only: false,
        });
        for line in lines {
            send(CommandPhase::Output {
                line: line.clone(),
                is_stderr: exit_code != 0,
            });
        }
        send(CommandPhase::Finished {
            exit_code: Some(exit_code),
            duration,
        });
    }

    fn emit(&self, id: CommandId, cmd: &GitCommand, phase: CommandPhase) {
        if let Some(bus) = &self.bus {
            bus.publish(CoreEvent::Command(CommandEvent {
                id,
                repo: cmd.repo,
                phase,
            }));
        }
    }

    /// Returns a closure that forwards one output line to the event bus.
    fn line_sink(
        &self,
        id: CommandId,
        cmd: &GitCommand,
        is_stderr: bool,
    ) -> impl Fn(String) + Send + 'static {
        let bus = self.bus.clone();
        let repo = cmd.repo;
        move |line: String| {
            if let Some(bus) = &bus {
                bus.publish(CoreEvent::Command(CommandEvent {
                    id,
                    repo,
                    phase: CommandPhase::Output { line, is_stderr },
                }));
            }
        }
    }
}

async fn join<T>(task: tokio::task::JoinHandle<std::io::Result<T>>) -> Result<T> {
    task.await
        .map_err(|e| GfError::Other(format!("output reader failed: {e}")))?
        .map_err(GfError::Io)
}

/// Splits a byte stream into lines on `\n` and `\r` (git redraws progress
/// with `\r`). Empty segments are dropped.
#[derive(Debug, Default)]
struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    fn feed(&mut self, bytes: &[u8], mut emit: impl FnMut(String)) {
        for &b in bytes {
            if b == b'\n' || b == b'\r' {
                self.flush(&mut emit);
            } else {
                self.pending.push(b);
            }
        }
    }

    fn finish(&mut self, mut emit: impl FnMut(String)) {
        self.flush(&mut emit);
    }

    fn flush(&mut self, emit: &mut impl FnMut(String)) {
        if !self.pending.is_empty() {
            emit(String::from_utf8_lossy(&self.pending).into_owned());
            self.pending.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_cr_and_lf_across_chunks() {
        let mut out = Vec::new();
        let mut s = LineSplitter::default();
        s.feed(b"Receiving 10%\rRecei", |l| out.push(l));
        s.feed(b"ving 20%\r\nDone", |l| out.push(l));
        s.finish(|l| out.push(l));
        assert_eq!(out, ["Receiving 10%", "Receiving 20%", "Done"]);
    }
}
