//! Tag list order: newest version first.
//!
//! git lists tags by plain string order, which puts `v1.10.0` before
//! `v1.9.0`. Names that read as versions (`v1.2.3`, `1.10`, `2.0.0-rc.1`)
//! sort here by semantic-version precedence instead, so the latest release
//! comes first and a pre-release sits below its release. Every other name
//! follows the versions, in natural order (`build-10` above `build-9`).
//!
//! The order is computed from names only, so it is the same for both git
//! backends and does not move when a tag is re-created.

use std::cmp::{Ordering, Reverse};

use super::Tag;

/// Sorts tags for display: versions newest first, then other names.
pub fn sort_newest_first(tags: &mut [Tag]) {
    tags.sort_by_cached_key(|t| Reverse(Key::new(t.name.short())));
}

/// Ascending order of a tag name; lists use it reversed.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    // Variant order matters: every other name ranks below every version.
    // The name breaks ties (`v1.0` and `1.0.0`) so the order is stable.
    Other(Vec<Chunk>, String),
    Version(Version, String),
}

impl Key {
    fn new(name: &str) -> Self {
        match Version::parse(name) {
            Some(v) => Self::Version(v, name.to_owned()),
            None => Self::Other(chunks(name), name.to_owned()),
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    /// Numeric parts without trailing zeros, so `1.0` and `1.0.0` compare
    /// equal and a shorter list is the lower version.
    core: Vec<u64>,
    stage: Stage,
}

/// A pre-release ranks below the release with the same numbers.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Pre(Vec<Ident>),
    Release,
}

/// Pre-release identifier: numbers compare numerically and rank below text.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ident {
    Num(u64),
    Text(String),
}

impl Version {
    /// `[v]N(.N)*[-pre][+build]`; build metadata does not affect order.
    fn parse(name: &str) -> Option<Self> {
        let s = name.strip_prefix(['v', 'V']).unwrap_or(name);
        let s = match s.split_once('+') {
            Some((v, build)) => idents(build).map(|_| v)?,
            None => s,
        };
        let (core, stage) = match s.split_once('-') {
            Some((core, pre)) => (core, Stage::Pre(idents(pre)?)),
            None => (s, Stage::Release),
        };
        let mut core: Vec<u64> = core.split('.').map(number).collect::<Option<_>>()?;
        while core.last() == Some(&0) {
            core.pop();
        }
        Some(Self { core, stage })
    }
}

fn number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Dot-separated identifiers of ASCII letters, digits and hyphens.
fn idents(s: &str) -> Option<Vec<Ident>> {
    s.split('.')
        .map(|id| {
            let valid =
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            valid.then(|| number(id).map_or_else(|| Ident::Text(id.to_owned()), Ident::Num))
        })
        .collect()
}

/// Runs of digits and of other characters, for natural order.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Chunk {
    Num(Digits),
    Text(String),
}

/// A run of digits compared by value, whatever its length.
#[derive(Debug, PartialEq, Eq)]
struct Digits(String);

impl Ord for Digits {
    fn cmp(&self, other: &Self) -> Ordering {
        let (a, b) = (
            self.0.trim_start_matches('0'),
            other.0.trim_start_matches('0'),
        );
        a.len().cmp(&b.len()).then_with(|| a.cmp(b))
    }
}

impl PartialOrd for Digits {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn chunks(name: &str) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut rest = name;
    while let Some(first) = rest.chars().next() {
        let digit = first.is_ascii_digit();
        let end = rest
            .find(|c: char| c.is_ascii_digit() != digit)
            .unwrap_or(rest.len());
        let (run, tail) = rest.split_at(end);
        out.push(if digit {
            Chunk::Num(Digits(run.to_owned()))
        } else {
            Chunk::Text(run.to_owned())
        });
        rest = tail;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Oid, RefName};

    fn sorted(names: &[&str]) -> Vec<String> {
        let target: Oid = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3"
            .parse()
            .expect("oid");
        let mut tags: Vec<Tag> = names
            .iter()
            .map(|n| Tag {
                name: RefName::tag(n),
                target: target.clone(),
                message: None,
            })
            .collect();
        sort_newest_first(&mut tags);
        tags.iter().map(|t| t.name.short().to_owned()).collect()
    }

    #[test]
    fn versions_sort_by_number_newest_first() {
        assert_eq!(
            sorted(&["v1.2.0", "v1.10.0", "v1.9.3", "v2.0.0", "v0.9"]),
            ["v2.0.0", "v1.10.0", "v1.9.3", "v1.2.0", "v0.9"]
        );
    }

    #[test]
    fn pre_releases_sit_below_their_release() {
        assert_eq!(
            sorted(&[
                "1.0.0-rc.2",
                "1.0.0",
                "1.0.0-rc.10",
                "1.0.0-alpha",
                "1.0.0-alpha.1",
                "0.9.0"
            ]),
            [
                "1.0.0",
                "1.0.0-rc.10",
                "1.0.0-rc.2",
                "1.0.0-alpha.1",
                "1.0.0-alpha",
                "0.9.0"
            ]
        );
    }

    #[test]
    fn numeric_pre_release_parts_rank_below_text() {
        assert_eq!(
            sorted(&["1.0.0-beta", "1.0.0-1"]),
            ["1.0.0-beta", "1.0.0-1"]
        );
    }

    #[test]
    fn short_versions_and_build_metadata_compare_by_numbers() {
        assert_eq!(
            sorted(&["v1", "v1.1", "v1.0.1", "v2+build.5"]),
            ["v2+build.5", "v1.1", "v1.0.1", "v1"]
        );
        // `1.0` and `1.0.0` are the same version; the name keeps the order stable.
        assert_eq!(sorted(&["v1.0", "1.0.0"]), sorted(&["1.0.0", "v1.0"]));
    }

    #[test]
    fn other_names_follow_versions_in_natural_order() {
        assert_eq!(
            sorted(&["build-9", "v1.0.0", "light", "build-10", "release/v2.0.0"]),
            ["v1.0.0", "release/v2.0.0", "light", "build-10", "build-9"]
        );
    }

    #[test]
    fn malformed_versions_are_other_names() {
        for name in [
            "v",
            "1..2",
            "1.2-",
            "1.2-rc..1",
            "1.2+",
            "v1.x",
            "version-1",
        ] {
            assert!(Version::parse(name).is_none(), "{name}");
        }
    }

    #[test]
    fn huge_numbers_fall_back_to_natural_order() {
        assert_eq!(
            sorted(&["99999999999999999999999", "100000000000000000000000"]),
            ["100000000000000000000000", "99999999999999999999999"]
        );
    }
}
