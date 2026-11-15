//! `git for-each-ref` output for branches and tags.
//!
//! Fields are NUL-separated and each record ends with 0x1e + newline, so
//! multi-line tag messages stay intact.

use gf_core::model::{Branch, RefName, Tag, Upstream};
use gf_core::{GfError, Result};

const RECORD_END: &[u8] = b"\x1e\n";

pub const BRANCH_ARGS: &[&str] = &[
    "for-each-ref",
    "--format=%(refname)%00%(objectname)%00%(HEAD)%00%(symref)%00%(upstream)%00%(upstream:track,nobracket)%1e",
    "refs/heads",
    "refs/remotes",
];

pub const TAG_ARGS: &[&str] = &[
    "for-each-ref",
    "--format=%(refname)%00%(objecttype)%00%(objectname)%00%(*objectname)%00%(contents)%1e",
    "refs/tags",
];

fn records(out: &[u8]) -> impl Iterator<Item = Vec<&[u8]>> {
    let mut rest = out;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = rest
            .windows(RECORD_END.len())
            .position(|w| w == RECORD_END)
            .unwrap_or(rest.len());
        let rec = &rest[..end];
        rest = rest.get(end + RECORD_END.len()..).unwrap_or_default();
        Some(rec.split(|&b| b == 0).collect())
    })
}

fn text(field: &[u8]) -> String {
    String::from_utf8_lossy(field).into_owned()
}

pub fn parse_branches(out: &[u8]) -> Result<Vec<Branch>> {
    let mut branches = Vec::new();
    for f in records(out) {
        let [name, oid, head, symref, upstream, track] = f.as_slice() else {
            return Err(GfError::parse(
                "branches",
                format!("expected 6 fields, got {}", f.len()),
            ));
        };
        // `refs/remotes/origin/HEAD` is a pointer to another branch, not a branch.
        if !symref.is_empty() {
            continue;
        }
        let upstream = (!upstream.is_empty())
            .then(|| {
                parse_track(&text(track)).map(|(ahead, behind, gone)| Upstream {
                    name: RefName::new(text(upstream)),
                    ahead,
                    behind,
                    gone,
                })
            })
            .transpose()?;
        branches.push(Branch {
            name: RefName::new(text(name)),
            target: text(oid).parse()?,
            upstream,
            is_head: *head == b"*",
        });
    }
    Ok(branches)
}

/// Parses `%(upstream:track,nobracket)`: `""`, `"gone"`, `"ahead 1"`,
/// `"behind 2"` or `"ahead 1, behind 2"`.
fn parse_track(track: &str) -> Result<(u32, u32, bool)> {
    let (mut ahead, mut behind) = (0, 0);
    if track == "gone" {
        return Ok((0, 0, true));
    }
    for part in track.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (key, n) = part
            .split_once(' ')
            .ok_or_else(|| GfError::parse("upstream track", track.to_owned()))?;
        let n: u32 = n
            .parse()
            .map_err(|_| GfError::parse("upstream track", track.to_owned()))?;
        match key {
            "ahead" => ahead = n,
            "behind" => behind = n,
            _ => return Err(GfError::parse("upstream track", track.to_owned())),
        }
    }
    Ok((ahead, behind, false))
}

pub fn parse_tags(out: &[u8]) -> Result<Vec<Tag>> {
    let mut tags = Vec::new();
    for f in records(out) {
        let [name, objtype, oid, peeled, contents] = f.as_slice() else {
            return Err(GfError::parse(
                "tags",
                format!("expected 5 fields, got {}", f.len()),
            ));
        };
        let annotated = *objtype == b"tag";
        let target = if annotated && !peeled.is_empty() {
            peeled
        } else {
            oid
        };
        tags.push(Tag {
            name: RefName::new(text(name)),
            target: text(target).parse()?,
            message: annotated.then(|| text(contents).trim_end().to_owned()),
        });
    }
    Ok(tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
    const B: &str = "b94a8fe5ccb19ba61c4c0873d391e987982fbbd3";

    #[test]
    fn parses_branches() {
        let out = format!(
            "refs/heads/main\0{A}\0*\0\0refs/remotes/origin/main\0ahead 1, behind 2\x1e\n\
             refs/heads/old\0{B}\0 \0\0refs/remotes/origin/old\0gone\x1e\n\
             refs/heads/local\0{B}\0 \0\0\0\x1e\n\
             refs/remotes/origin/HEAD\0{A}\0 \0refs/remotes/origin/main\0\0\x1e\n\
             refs/remotes/origin/main\0{A}\0 \0\0\0\x1e\n"
        );
        let branches = parse_branches(out.as_bytes()).expect("parse");
        assert_eq!(branches.len(), 4, "symbolic origin/HEAD is skipped");

        let main = &branches[0];
        assert!(main.is_head);
        let up = main.upstream.as_ref().expect("upstream");
        assert_eq!(up.name.short(), "origin/main");
        assert_eq!((up.ahead, up.behind, up.gone), (1, 2, false));

        assert!(branches[1].upstream.as_ref().is_some_and(|u| u.gone));
        assert!(branches[2].upstream.is_none());
        assert_eq!(branches[3].name.short(), "origin/main");
    }

    #[test]
    fn parses_annotated_and_lightweight_tags() {
        let out = format!(
            "refs/tags/v1\0tag\0{B}\0{A}\0Release 1\n\nNotes line\n\x1e\n\
             refs/tags/light\0commit\0{A}\0\0\x1e\n"
        );
        let tags = parse_tags(out.as_bytes()).expect("parse");
        assert_eq!(tags[0].target.as_str(), A, "annotated tag is peeled");
        assert_eq!(tags[0].message.as_deref(), Some("Release 1\n\nNotes line"));
        assert_eq!(tags[1].target.as_str(), A);
        assert_eq!(tags[1].message, None);
    }

    #[test]
    fn track_variants() {
        assert_eq!(parse_track("").ok(), Some((0, 0, false)));
        assert_eq!(parse_track("behind 4").ok(), Some((0, 4, false)));
        assert!(parse_track("sideways 1").is_err());
    }
}
