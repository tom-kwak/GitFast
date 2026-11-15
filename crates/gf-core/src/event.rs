//! Process-wide event bus. Producers (git runner, file watcher, hosting sync)
//! publish [`CoreEvent`]s; UI features subscribe and refresh what they show.

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::broadcast;

use crate::model::RepoId;

/// Identifier of a single git command execution, used to correlate the
/// started/output/finished events shown in the command log.
pub type CommandId = u64;

#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent {
    RepoOpened(RepoId),
    RepoClosed(RepoId),
    /// Files in the working tree changed on disk.
    WorkTreeChanged {
        repo: RepoId,
        paths: Vec<PathBuf>,
    },
    /// `.git/index` changed (stage/unstage, checkout, ...).
    IndexChanged(RepoId),
    /// Branches, tags, HEAD or remote refs changed.
    RefsChanged(RepoId),
    ConfigChanged(RepoId),
    Command(CommandEvent),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandEvent {
    pub id: CommandId,
    pub repo: Option<RepoId>,
    pub phase: CommandPhase,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandPhase {
    Started {
        argv: Vec<String>,
        /// Background read issued by gf itself (status refresh, log...)
        /// rather than a user action. UIs may show these less prominently.
        read_only: bool,
    },
    /// One line of stdout/stderr, forwarded for live progress display.
    Output { line: String, is_stderr: bool },
    Finished {
        exit_code: Option<i32>,
        duration: Duration,
    },
}

/// Cheap-to-clone handle to the broadcast channel.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<CoreEvent>,
}

impl EventBus {
    /// `capacity` is how many events a slow subscriber may lag behind before
    /// it starts missing events (it then receives `RecvError::Lagged`).
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publishes an event. Having no subscribers is not an error.
    pub fn publish(&self, event: CoreEvent) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_receive_published_events() {
        let bus = EventBus::default();
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        let repo = RepoId::new();

        bus.publish(CoreEvent::RefsChanged(repo));

        assert_eq!(a.recv().await.ok(), Some(CoreEvent::RefsChanged(repo)));
        assert_eq!(b.recv().await.ok(), Some(CoreEvent::RefsChanged(repo)));
    }

    #[test]
    fn publishing_without_subscribers_is_fine() {
        EventBus::default().publish(CoreEvent::RepoClosed(RepoId::new()));
    }
}
