use std::fmt;

use super::Oid;

/// A full ref name such as `refs/heads/main`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RefName(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
    Stash,
    Note,
    Other,
}

impl RefName {
    pub fn new(full: impl Into<String>) -> Self {
        Self(full.into())
    }

    pub fn local_branch(short: &str) -> Self {
        Self(format!("refs/heads/{short}"))
    }

    pub fn tag(short: &str) -> Self {
        Self(format!("refs/tags/{short}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn kind(&self) -> RefKind {
        let s = self.0.as_str();
        if s == "HEAD" {
            RefKind::Head
        } else if s.starts_with("refs/heads/") {
            RefKind::LocalBranch
        } else if s.starts_with("refs/remotes/") {
            RefKind::RemoteBranch
        } else if s.starts_with("refs/tags/") {
            RefKind::Tag
        } else if s == "refs/stash" {
            RefKind::Stash
        } else if s.starts_with("refs/notes/") {
            RefKind::Note
        } else {
            RefKind::Other
        }
    }

    /// Name as users type it: `main`, `origin/main`, `v1.0`.
    pub fn short(&self) -> &str {
        let s = self.0.as_str();
        [
            "refs/heads/",
            "refs/remotes/",
            "refs/tags/",
            "refs/notes/",
            "refs/",
        ]
        .iter()
        .find_map(|prefix| s.strip_prefix(prefix))
        .unwrap_or(s)
    }
}

impl fmt::Display for RefName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub name: RefName,
    pub ahead: u32,
    pub behind: u32,
    /// The upstream is configured but its remote ref no longer exists.
    pub gone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: RefName,
    pub target: Oid,
    pub upstream: Option<Upstream>,
    pub is_head: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: RefName,
    /// The commit the tag ultimately points to (peeled).
    pub target: Oid,
    /// Message of an annotated tag; `None` for lightweight tags.
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_and_shortens_refs() {
        let cases = [
            ("HEAD", RefKind::Head, "HEAD"),
            (
                "refs/heads/feature/login",
                RefKind::LocalBranch,
                "feature/login",
            ),
            (
                "refs/remotes/origin/main",
                RefKind::RemoteBranch,
                "origin/main",
            ),
            ("refs/tags/v1.0.0", RefKind::Tag, "v1.0.0"),
            ("refs/stash", RefKind::Stash, "stash"),
            ("refs/notes/commits", RefKind::Note, "commits"),
            (
                "refs/merge-requests/1/head",
                RefKind::Other,
                "merge-requests/1/head",
            ),
        ];
        for (full, kind, short) in cases {
            let r = RefName::new(full);
            assert_eq!(r.kind(), kind, "{full}");
            assert_eq!(r.short(), short, "{full}");
        }
    }

    #[test]
    fn constructors_build_full_names() {
        assert_eq!(RefName::local_branch("main").as_str(), "refs/heads/main");
        assert_eq!(RefName::tag("v1").as_str(), "refs/tags/v1");
    }
}
