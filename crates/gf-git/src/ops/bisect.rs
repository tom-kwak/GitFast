//! `git bisect`: binary search for the commit that introduced a bug.

use gf_core::Result;
use gf_core::model::Oid;

use crate::backend::{BisectOps, CliBackend};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Good,
    Bad,
    Skip,
}

/// What git said after a bisect step.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BisectProgress {
    /// Revisions left to test after the current one.
    pub remaining: Option<u32>,
    /// Roughly how many more steps.
    pub steps: Option<u32>,
    /// Set when the search is over.
    pub first_bad: Option<Oid>,
}

/// How git names the result: `bad` before 2.56, `'bad'` since (quoted,
/// as custom terms are).
const FIRST_BAD: [&str; 2] = ["first bad commit", "first 'bad' commit"];

/// Parses `Bisecting: 6 revisions left to test after this (roughly 3 steps)`
/// and `<oid> is the first bad commit` (or `first 'bad' commit`).
pub fn parse_progress(out: &str) -> BisectProgress {
    let mut p = BisectProgress::default();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("Bisecting: ") {
            p.remaining = rest.split_whitespace().next().and_then(|n| n.parse().ok());
            p.steps = rest
                .split("(roughly ")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|n| n.parse().ok());
        } else if let Some(id) = FIRST_BAD
            .iter()
            .find_map(|f| line.strip_suffix(&format!(" is the {f}")))
        {
            p.first_bad = id.trim().parse().ok();
        }
    }
    p
}

/// The `# first bad commit: [<oid>] <subject>` line of `git bisect log`
/// (`first 'bad' commit` since git 2.56).
pub fn first_bad_in_log(log: &str) -> Option<Oid> {
    log.lines()
        .find_map(|l| {
            FIRST_BAD
                .iter()
                .find_map(|f| l.strip_prefix(&format!("# {f}: [")))
        })
        .and_then(|rest| rest.split(']').next())
        .and_then(|id| id.parse().ok())
}

impl BisectOps for CliBackend {
    async fn bisect_start(&self, bad: &str, good: &str) -> Result<BisectProgress> {
        // `bisect start` has no `--end-of-options`; refuse option-like names.
        if bad.starts_with('-') || good.starts_with('-') {
            return Err(gf_core::GfError::Other(format!(
                "not a revision: {bad} / {good}"
            )));
        }
        let out = self
            .exec(self.command(["bisect", "start", bad, good]))
            .await?;
        Ok(parse_progress(&out.stdout_str()))
    }

    async fn bisect_mark(&self, verdict: Verdict) -> Result<BisectProgress> {
        let word = match verdict {
            Verdict::Good => "good",
            Verdict::Bad => "bad",
            Verdict::Skip => "skip",
        };
        let out = self.exec(self.command(["bisect", word])).await?;
        let mut p = parse_progress(&out.stdout_str());
        if p.first_bad.is_none() && p.remaining.is_none() {
            // Neither "Bisecting:" nor a result (wording git changed
            // again?): the bisect log records the result too.
            let log = self.exec(self.command(["bisect", "log"])).await?;
            p.first_bad = first_bad_in_log(&log.stdout_str());
        }
        Ok(p)
    }

    async fn bisect_reset(&self) -> Result<()> {
        self.exec(self.command(["bisect", "reset"])).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_and_result() {
        let p = parse_progress(
            "Bisecting: 6 revisions left to test after this (roughly 3 steps)\n[abc] msg\n",
        );
        assert_eq!(
            (p.remaining, p.steps, p.first_bad.is_some()),
            (Some(6), Some(3), false)
        );
        let id = "c".repeat(40);
        let p = parse_progress(&format!("{id} is the first bad commit\ncommit {id}\n"));
        assert_eq!(p.first_bad.map(|o| o.to_string()), Some(id.clone()));
        let log = format!("git bisect good abc\n# first bad commit: [{id}] c5\n");
        assert_eq!(
            first_bad_in_log(&log).map(|o| o.to_string()),
            Some(id.clone())
        );
        assert_eq!(first_bad_in_log("git bisect start\n"), None);
        // git 2.56 quotes the term.
        let p = parse_progress(&format!("{id} is the first 'bad' commit\ncommit {id}\n"));
        assert_eq!(p.first_bad.map(|o| o.to_string()), Some(id.clone()));
        let log = format!("# first 'bad' commit: [{id}] c5\n");
        assert_eq!(first_bad_in_log(&log).map(|o| o.to_string()), Some(id));
    }
}
