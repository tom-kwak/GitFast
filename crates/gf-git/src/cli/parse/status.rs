//! `git status --porcelain=v2 -z --branch`.

use gf_core::model::{ChangeKind, ConflictKind, HeadInfo, StatusEntry, StatusReport};
use gf_core::{GfError, Result};

use super::util::{bytes_to_path, split_records};

pub const STATUS_ARGS: &[&str] = &[
    "status",
    "--porcelain=v2",
    "-z",
    "--branch",
    "--untracked-files=all",
    "--ignore-submodules=none",
];

pub fn parse_status(out: &[u8]) -> Result<StatusReport> {
    let mut report = StatusReport::default();
    let mut records = split_records(out, 0);

    while let Some(rec) = records.next() {
        let line = std::str::from_utf8(rec).ok();
        match rec.first() {
            Some(b'#') => parse_header(rec, &mut report.head)?,
            Some(b'1') => report.entries.push(parse_changed(rec, None)?),
            Some(b'2') => {
                // The original path follows as its own NUL-terminated record.
                let orig = records
                    .next()
                    .ok_or_else(|| GfError::parse("status", "rename without source path"))?;
                report.entries.push(parse_changed(rec, Some(orig))?);
            }
            Some(b'u') => report.entries.push(parse_unmerged(rec)?),
            Some(b'?') => report.entries.push(StatusEntry::Untracked {
                path: bytes_to_path(rest_after(rec, 1)?),
            }),
            Some(b'!') => report.entries.push(StatusEntry::Ignored {
                path: bytes_to_path(rest_after(rec, 1)?),
            }),
            _ => {
                return Err(GfError::parse(
                    "status",
                    format!("unknown record: {}", line.unwrap_or("<binary>")),
                ));
            }
        }
    }
    Ok(report)
}

fn parse_header(rec: &[u8], head: &mut HeadInfo) -> Result<()> {
    let line =
        std::str::from_utf8(rec).map_err(|e| GfError::parse("status header", e.to_string()))?;
    let Some((key, value)) = line.trim_start_matches("# ").split_once(' ') else {
        return Ok(());
    };
    match key {
        "branch.oid" if value != "(initial)" => head.oid = Some(value.parse()?),
        "branch.head" if value != "(detached)" => head.branch = Some(value.to_owned()),
        "branch.upstream" => head.upstream = Some(value.to_owned()),
        "branch.ab" => {
            for part in value.split_whitespace() {
                let n = |s: &str| {
                    s.parse::<u32>()
                        .map_err(|e| GfError::parse("status branch.ab", e.to_string()))
                };
                if let Some(a) = part.strip_prefix('+') {
                    head.ahead = n(a)?;
                } else if let Some(b) = part.strip_prefix('-') {
                    head.behind = n(b)?;
                }
            }
        }
        // `# stash N` and future headers are ignored.
        _ => {}
    }
    Ok(())
}

/// `1 XY sub mH mI mW hH hI path`
/// `2 XY sub mH mI mW hH hI Xscore path` (+ separate orig path record)
fn parse_changed(rec: &[u8], orig: Option<&[u8]>) -> Result<StatusEntry> {
    let fields_before_path = if orig.is_some() { 9 } else { 8 };
    let xy = field(rec, 1)?;
    let (index, worktree) = parse_xy(xy)?;
    Ok(StatusEntry::Changed {
        path: bytes_to_path(rest_after(rec, fields_before_path)?),
        orig_path: orig.map(bytes_to_path),
        index,
        worktree,
    })
}

/// `u XY sub m1 m2 m3 mW h1 h2 h3 path`
fn parse_unmerged(rec: &[u8]) -> Result<StatusEntry> {
    let xy = std::str::from_utf8(field(rec, 1)?).unwrap_or_default();
    let kind = ConflictKind::from_porcelain(xy)
        .ok_or_else(|| GfError::parse("status", format!("unknown conflict code {xy}")))?;
    Ok(StatusEntry::Conflicted {
        path: bytes_to_path(rest_after(rec, 10)?),
        kind,
    })
}

fn parse_xy(xy: &[u8]) -> Result<(ChangeKind, ChangeKind)> {
    let bad = || {
        GfError::parse(
            "status",
            format!("bad XY {:?}", String::from_utf8_lossy(xy)),
        )
    };
    let [x, y] = xy else { return Err(bad()) };
    Ok((
        ChangeKind::from_porcelain(char::from(*x)).ok_or_else(bad)?,
        ChangeKind::from_porcelain(char::from(*y)).ok_or_else(bad)?,
    ))
}

/// The n-th space-separated field (0-based).
fn field(rec: &[u8], n: usize) -> Result<&[u8]> {
    rec.split(|&b| b == b' ')
        .nth(n)
        .ok_or_else(|| GfError::parse("status", "missing field"))
}

/// Everything after the first `n` space-separated fields. Paths may contain
/// spaces, so they are never split.
fn rest_after(rec: &[u8], n: usize) -> Result<&[u8]> {
    let mut start = 0;
    for _ in 0..n {
        let space = rec[start..]
            .iter()
            .position(|&b| b == b' ')
            .ok_or_else(|| GfError::parse("status", "missing path"))?;
        start += space + 1;
    }
    Ok(&rec[start..])
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const OID: &str = "1111111111111111111111111111111111111111";

    fn sample() -> Vec<u8> {
        let records = [
            format!("# branch.oid {OID}"),
            "# branch.head feature/x".into(),
            "# branch.upstream origin/feature/x".into(),
            "# branch.ab +2 -3".into(),
            format!("1 .M N... 100644 100644 100644 {OID} {OID} src/main.rs"),
            format!("1 A. N... 000000 100644 100644 {OID} {OID} dir with space/new file.txt"),
            format!("2 R. N... 100644 100644 100644 {OID} {OID} R100 new name.rs"),
            "old name.rs".into(),
            format!("u UU N... 100644 100644 100644 100644 {OID} {OID} {OID} conflict.rs"),
            "? untracked é.txt".into(),
            "! target/".into(),
        ];
        records.iter().flat_map(|r| r.bytes().chain([0])).collect()
    }

    #[test]
    fn parses_headers() {
        let report = parse_status(&sample()).expect("parse");
        assert_eq!(report.head.oid, OID.parse().ok());
        assert_eq!(report.head.branch.as_deref(), Some("feature/x"));
        assert_eq!(report.head.upstream.as_deref(), Some("origin/feature/x"));
        assert_eq!((report.head.ahead, report.head.behind), (2, 3));
    }

    #[test]
    fn parses_every_entry_kind() {
        let report = parse_status(&sample()).expect("parse");
        assert_eq!(
            report.entries,
            vec![
                StatusEntry::Changed {
                    path: "src/main.rs".into(),
                    orig_path: None,
                    index: ChangeKind::Unmodified,
                    worktree: ChangeKind::Modified,
                },
                StatusEntry::Changed {
                    path: "dir with space/new file.txt".into(),
                    orig_path: None,
                    index: ChangeKind::Added,
                    worktree: ChangeKind::Unmodified,
                },
                StatusEntry::Changed {
                    path: "new name.rs".into(),
                    orig_path: Some(PathBuf::from("old name.rs")),
                    index: ChangeKind::Renamed,
                    worktree: ChangeKind::Unmodified,
                },
                StatusEntry::Conflicted {
                    path: "conflict.rs".into(),
                    kind: ConflictKind::BothModified,
                },
                StatusEntry::Untracked {
                    path: "untracked é.txt".into()
                },
                StatusEntry::Ignored {
                    path: "target/".into()
                },
            ]
        );
    }

    #[test]
    fn initial_and_detached_heads() {
        let out = b"# branch.oid (initial)\0# branch.head (detached)\0";
        let report = parse_status(out).expect("parse");
        assert_eq!(report.head, HeadInfo::default());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_status(b"x what\0").is_err());
        assert!(parse_status(b"2 R. N... 1 1 1 a b R100 x\0").is_err());
    }

    #[test]
    fn empty_output_is_clean_tree() {
        assert_eq!(parse_status(b"").expect("parse"), StatusReport::default());
    }
}
