//! Which tags a remote has, as of the last fetch or push.
//!
//! Unlike branches, git keeps no record of a remote's tags: once fetched,
//! a remote tag is an ordinary local tag. To tell which local tags a push
//! would publish, gf lists each remote's tags (`git ls-remote --tags`)
//! after a fetch, adds the ones it pushes, and keeps the names in
//! `<common git dir>/gf/remote-tags/<remote>`, one per line (shared by
//! linked worktrees, like the tags and remotes themselves). Reading the file is
//! cheap and needs no network; a remote never fetched has no file, and its
//! tags are unknown (`None`), not "none".

use std::collections::BTreeSet;
use std::path::PathBuf;

use gf_core::Result;

use crate::Repository;

impl Repository {
    /// The tag names `remote` had at the last fetch or push through gf;
    /// `None` when gf never listed them.
    pub async fn known_remote_tags(&self, remote: &str) -> Option<BTreeSet<String>> {
        let text = tokio::fs::read_to_string(self.remote_tags_file(remote).await)
            .await
            .ok()?;
        Some(
            text.lines()
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    }

    /// Lists `remote`'s tags over the network and records them.
    pub async fn record_remote_tags(&self, remote: &str) -> Result<()> {
        let cmd = self
            .cli
            .command(["ls-remote", "--tags", "--refs", "--end-of-options", remote]);
        let out = self.cli.read(cmd).await?;
        let names = parse_ls_remote_tags(&String::from_utf8_lossy(&out));
        self.write_remote_tags(remote, &names).await
    }

    /// Adds `names` to what `remote` is known to have (after pushing them).
    /// Without an earlier listing nothing is recorded: the rest is unknown.
    pub async fn add_remote_tags(&self, remote: &str, names: &[String]) -> Result<()> {
        let Some(mut known) = self.known_remote_tags(remote).await else {
            return Ok(());
        };
        known.extend(names.iter().cloned());
        self.write_remote_tags(remote, &known).await
    }

    /// Pushes `names` (tags) to `remote` and records them as published.
    pub async fn push_tags(&self, remote: &str, names: &[String]) -> Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        let mut args = vec!["push".to_owned(), remote.to_owned()];
        args.extend(names.iter().map(|n| format!("refs/tags/{n}")));
        self.cli.exec(self.cli.command(args)).await?;
        self.add_remote_tags(remote, names).await
    }

    async fn write_remote_tags(&self, remote: &str, names: &BTreeSet<String>) -> Result<()> {
        let path = self.remote_tags_file(remote).await;
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let mut text = String::new();
        for n in names {
            text.push_str(n);
            text.push('\n');
        }
        // Written aside and renamed: a refresh reading it meanwhile sees
        // the old list or the new one, never half of it.
        // Unique per write: a fetch and a background listing may both write.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_extension(format!("tmp-{}-{n}", std::process::id()));
        tokio::fs::write(&tmp, text).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }

    async fn remote_tags_file(&self, remote: &str) -> PathBuf {
        // Remote names may contain `/` (and `.`): one flat file per remote.
        let file = remote
            .replace('%', "%25")
            .replace('/', "%2F")
            .replace('.', "%2E");
        common_dir(&self.handle().git_dir)
            .await
            .join("gf")
            .join("remote-tags")
            .join(file)
    }
}

/// The git dir shared by all worktrees: a linked worktree's git dir names
/// it in its `commondir` file.
async fn common_dir(git_dir: &std::path::Path) -> PathBuf {
    match tokio::fs::read_to_string(git_dir.join("commondir")).await {
        Ok(rel) => git_dir.join(rel.trim()),
        Err(_) => git_dir.to_path_buf(),
    }
}

/// Tag names from `git ls-remote --tags --refs` (`<oid>\trefs/tags/<name>`).
fn parse_ls_remote_tags(out: &str) -> BTreeSet<String> {
    out.lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(_, r)| r.strip_prefix("refs/tags/"))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_remote_lines_give_tag_names() {
        let out = "aaa\trefs/tags/v1\nbbb\trefs/tags/release/2\nccc\trefs/heads/main\n";
        let names: Vec<_> = parse_ls_remote_tags(out).into_iter().collect();
        assert_eq!(names, ["release/2", "v1"]);
    }
}
