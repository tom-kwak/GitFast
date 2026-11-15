//! Creating commits.

use gf_core::Result;
use gf_core::model::Oid;

use crate::Repository;

#[derive(Debug, Clone, Default)]
pub struct CommitOptions {
    /// Replace the last commit instead of creating a new one.
    pub amend: bool,
    /// Add a `Signed-off-by` trailer.
    pub sign_off: bool,
    /// Allow a commit without changes.
    pub allow_empty: bool,
}

impl Repository {
    /// Commits the index with `message` (passed on stdin, so any text is
    /// safe). Returns the new commit id. Hooks run as usual.
    pub async fn commit(&self, message: &str, opts: &CommitOptions) -> Result<Oid> {
        // `whitespace` cleanup keeps lines starting with `#` (issue numbers).
        let mut cmd = self.command(["commit", "--cleanup=whitespace", "-F", "-"]);
        if opts.amend {
            cmd = cmd.arg("--amend");
        }
        if opts.sign_off {
            cmd = cmd.arg("--signoff");
        }
        if opts.allow_empty {
            cmd = cmd.arg("--allow-empty");
        }
        self.exec(cmd.stdin(message.as_bytes().to_vec())).await?;
        let head = self.read(self.command(["rev-parse", "HEAD"])).await?;
        String::from_utf8_lossy(&head).trim().parse()
    }

    /// Full message of HEAD, e.g. to prefill an amend. `None` before the
    /// first commit.
    pub async fn head_message(&self) -> Result<Option<String>> {
        if !self.head_exists().await? {
            return Ok(None);
        }
        let out = self
            .read(self.command(["log", "-1", "--format=%B", "HEAD"]))
            .await?;
        Ok(Some(String::from_utf8_lossy(&out).trim_end().to_owned()))
    }
}
