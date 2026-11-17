//! Housekeeping that makes later reads faster without changing what they
//! return: the commit-graph file.
//!
//! Without a commit-graph, `git log --topo-order` (the history graph) must
//! walk every commit before it prints the first one, and gix falls back to
//! git for it (see `GixBackend::log`). Git writes the file itself during
//! `git gc` / `git maintenance`, so many repositories already have one.

use gf_core::Result;

use crate::backend::{CliBackend, MaintenanceOps};

/// Whether a commit-graph should be written, from `git rev-parse` output
/// (`--is-shallow-repository`, then the two `--git-path` lines) and
/// whether those paths exist, plus `core.commitGraph`.
pub fn wants_commit_graph(shallow: bool, file_exists: bool, enabled: bool) -> bool {
    !shallow && !file_exists && enabled
}

impl MaintenanceOps for CliBackend {
    async fn write_commit_graph_if_missing(&self) -> Result<bool> {
        let out = self
            .read(self.command([
                "rev-parse",
                "--is-shallow-repository",
                "--git-path",
                "objects/info/commit-graph",
                "--git-path",
                "objects/info/commit-graphs/commit-graph-chain",
            ]))
            .await?;
        let text = String::from_utf8_lossy(&out);
        let mut lines = text.lines();
        let shallow = lines.next() == Some("true");
        let cwd = self.cwd().to_path_buf();
        let file_exists = lines.any(|p| cwd.join(p).exists());
        // `core.commitGraph=false` turns the file off: writing it is useless.
        let enabled = self
            .runner()
            .run(
                &self
                    .command(["config", "--type=bool", "--get", "core.commitGraph"])
                    .read_only()
                    .allow_failure(),
            )
            .await
            .map(|o| o.stdout_str().trim() != "false")
            .unwrap_or(true);
        if !wants_commit_graph(shallow, file_exists, enabled) {
            return Ok(false);
        }
        self.exec(self.command(["commit-graph", "write", "--reachable", "--no-progress"]))
            .await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_only_when_missing_enabled_and_not_shallow() {
        assert!(wants_commit_graph(false, false, true));
        assert!(!wants_commit_graph(true, false, true), "shallow");
        assert!(!wants_commit_graph(false, true, true), "already there");
        assert!(!wants_commit_graph(false, false, false), "turned off");
    }
}
