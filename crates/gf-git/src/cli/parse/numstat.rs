//! `git diff --numstat -z`: added/removed line counts per file.

use std::collections::HashMap;
use std::path::PathBuf;

use gf_core::{GfError, Result};

use super::util::bytes_to_path;

/// Flags producing the format [`parse_numstat`] reads; add `--cached` for
/// staged changes.
pub const NUMSTAT_ARGS: &[&str] = &[
    "diff",
    "--numstat",
    "-z",
    "--no-color",
    "--no-ext-diff",
    "--no-relative",
    "--find-renames",
];

/// Lines added and removed in one file. `None` from the parser means a
/// binary file (git reports `-` for both).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineCounts {
    pub added: u32,
    pub removed: u32,
}

/// Parses `-z` numstat output keyed by the (new) path. Records are
/// `added\tremoved\tpath\0`, or for renames `added\tremoved\t\0old\0new\0`.
pub fn parse_numstat(out: &[u8]) -> Result<HashMap<PathBuf, Option<LineCounts>>> {
    let mut map = HashMap::new();
    let mut fields = out.split(|&b| b == 0).peekable();
    while let Some(rec) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        let bad = || GfError::parse("numstat", String::from_utf8_lossy(rec).into_owned());
        let mut parts = rec.splitn(3, |&b| b == b'\t');
        let (Some(a), Some(r), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(bad());
        };
        let path = if path.is_empty() {
            // Rename/copy: the old and new paths follow as their own fields.
            let _old = fields.next().ok_or_else(bad)?;
            fields.next().ok_or_else(bad)?
        } else {
            path
        };
        let num = |s: &[u8]| {
            std::str::from_utf8(s)
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
        };
        let counts = match (num(a), num(r)) {
            (Some(added), Some(removed)) => Some(LineCounts { added, removed }),
            _ if a == b"-" && r == b"-" => None,
            _ => return Err(bad()),
        };
        map.insert(bytes_to_path(path), counts);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_counts_binary_and_renames() {
        let out = b"3\t1\tsrc/main.rs\0-\t-\tlogo.png\x000\t0\t\0old name.rs\0new name.rs\0";
        let m = parse_numstat(out).expect("parse");
        assert_eq!(
            m[&PathBuf::from("src/main.rs")],
            Some(LineCounts {
                added: 3,
                removed: 1
            })
        );
        assert_eq!(m[&PathBuf::from("logo.png")], None);
        assert_eq!(
            m[&PathBuf::from("new name.rs")],
            Some(LineCounts::default())
        );
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_numstat(b"x\ty\tz\0").is_err());
        assert!(parse_numstat(b"").expect("empty").is_empty());
    }
}
