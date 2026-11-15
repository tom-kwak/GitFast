//! `git blame --porcelain`: who last changed each line.

use std::collections::HashMap;
use std::path::Path;

use gf_core::model::{Oid, Timestamp};
use gf_core::{GfError, Result};

use crate::Repository;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub commit: Oid,
    /// 1-based line number in the blamed revision.
    pub line: u32,
    pub content: String,
    pub author: String,
    pub time: Timestamp,
    pub summary: String,
}

#[derive(Debug, Clone, Default)]
struct CommitInfo {
    author: String,
    time: i64,
    tz: i32,
    summary: String,
}

fn parse_tz(tz: &str) -> i32 {
    let sign = if tz.starts_with('-') { -1 } else { 1 };
    let digits = tz.trim_start_matches(['+', '-']);
    let h: i32 = digits.get(..2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let m: i32 = digits.get(2..4).and_then(|s| s.parse().ok()).unwrap_or(0);
    sign * (h * 60 + m)
}

/// Parses porcelain output. Commit headers carry details only the first
/// time a commit appears, so they are remembered by id.
pub fn parse_blame(out: &[u8]) -> Result<Vec<BlameLine>> {
    let text = String::from_utf8_lossy(out);
    let mut infos: HashMap<String, CommitInfo> = HashMap::new();
    let mut lines = Vec::new();
    let mut current: Option<(String, u32)> = None;
    for raw in text.split('\n') {
        if let Some(content) = raw.strip_prefix('\t') {
            let (id, line) = current
                .take()
                .ok_or_else(|| GfError::parse("blame", "content line without header"))?;
            let info = infos.get(&id).cloned().unwrap_or_default();
            lines.push(BlameLine {
                commit: id.parse()?,
                line,
                content: content.to_owned(),
                author: info.author,
                time: Timestamp {
                    seconds: info.time,
                    offset_minutes: info.tz,
                },
                summary: info.summary,
            });
            continue;
        }
        let mut words = raw.splitn(2, ' ');
        let (Some(first), rest) = (words.next(), words.next()) else {
            continue;
        };
        let is_oid = (first.len() == 40 || first.len() == 64)
            && first.bytes().all(|b| b.is_ascii_hexdigit());
        if is_oid {
            // `<oid> <orig line> <final line> [group size]`
            let final_line = rest
                .and_then(|r| r.split(' ').nth(1))
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| GfError::parse("blame header", raw.to_owned()))?;
            infos.entry(first.to_owned()).or_default();
            current = Some((first.to_owned(), final_line));
            continue;
        }
        if let (Some((id, _)), Some(value)) = (&current, rest) {
            let info = infos.entry(id.clone()).or_default();
            match first {
                "author" => info.author = value.to_owned(),
                "author-time" => info.time = value.parse().unwrap_or(0),
                "author-tz" => info.tz = parse_tz(value),
                "summary" => info.summary = value.to_owned(),
                _ => {}
            }
        }
    }
    Ok(lines)
}

impl Repository {
    /// Blame of `path` at `rev` (`None` = the working tree version).
    pub async fn blame(&self, path: &Path, rev: Option<&str>) -> Result<Vec<BlameLine>> {
        let mut cmd = self.command(["blame", "--porcelain"]);
        if let Some(rev) = rev {
            cmd = cmd.arg(rev);
        }
        parse_blame(&self.read(cmd.paths([path])).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_with_repeated_commits() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let out = format!(
            "{a} 1 1 2\nauthor Kim\nauthor-time 1704067200\nauthor-tz +0900\nsummary first\nfilename f\n\tline one\n\
             {a} 2 2\n\tline two\n\
             {b} 3 3 1\nauthor Lee\nauthor-time 1704067300\nauthor-tz -0130\nsummary second\nfilename f\n\tline three\n"
        );
        let lines = parse_blame(out.as_bytes()).expect("parse");
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[1].author, "Kim",
            "details carried from the first header"
        );
        assert_eq!(lines[1].content, "line two");
        assert_eq!(lines[2].time.offset_minutes, -90);
        assert_eq!(lines[2].line, 3);
    }
}
