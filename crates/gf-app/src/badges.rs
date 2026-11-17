//! Branch and tag badges per commit (`HEAD → main`, `origin/main`,
//! `◆ v1.0`), built once per snapshot load off the UI thread instead of on
//! every frame. They carry a [`Kind`], not a color, so a theme switch needs
//! no reload: the view picks the color.

use std::collections::HashMap;

use gf_core::model::{Branch, HeadInfo, Oid, RefKind, Tag};

/// What a badge names; decides its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Head,
    Branch,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub label: String,
    pub kind: Kind,
}

/// Badges per commit, HEAD's first.
pub type Badges = HashMap<Oid, Vec<Badge>>;

/// The badges of `branches` and `tags`; a detached `head` gets a `HEAD`
/// badge of its own.
pub fn build(branches: &[Branch], tags: &[Tag], head: &HeadInfo) -> Badges {
    let mut map: Badges = HashMap::new();
    for b in branches {
        let (label, kind) = match b.name.kind() {
            RefKind::LocalBranch if b.is_head => (format!("HEAD → {}", b.name.short()), Kind::Head),
            RefKind::LocalBranch => (b.name.short().to_owned(), Kind::Branch),
            _ => (b.name.short().to_owned(), Kind::Remote),
        };
        let list = map.entry(b.target.clone()).or_default();
        let badge = Badge { label, kind };
        if b.is_head {
            list.insert(0, badge);
        } else {
            list.push(badge);
        }
    }
    for tag in tags {
        map.entry(tag.target.clone()).or_default().push(Badge {
            label: format!("◆ {}", tag.name.short()),
            kind: Kind::Tag,
        });
    }
    if head.branch.is_none()
        && let Some(oid) = &head.oid
    {
        map.entry(oid.clone()).or_default().insert(
            0,
            Badge {
                label: "HEAD".into(),
                kind: Kind::Head,
            },
        );
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use gf_core::model::RefName;

    fn oid(c: char) -> Oid {
        c.to_string().repeat(40).parse().expect("oid")
    }

    fn branch(name: &str, target: char, is_head: bool) -> Branch {
        Branch {
            name: RefName::new(name),
            target: oid(target),
            upstream: None,
            is_head,
        }
    }

    #[test]
    fn head_comes_first_then_branches_then_tags() {
        let branches = [
            branch("refs/remotes/origin/main", 'a', false),
            branch("refs/heads/main", 'a', true),
            branch("refs/heads/topic", 'b', false),
        ];
        let tags = [Tag {
            name: RefName::new("refs/tags/v1"),
            target: oid('a'),
            message: None,
        }];
        let head = HeadInfo {
            branch: Some("main".into()),
            oid: Some(oid('a')),
            ..HeadInfo::default()
        };
        let map = build(&branches, &tags, &head);
        let labels: Vec<_> = map[&oid('a')]
            .iter()
            .map(|b| (b.label.as_str(), b.kind))
            .collect();
        assert_eq!(
            labels,
            [
                ("HEAD → main", Kind::Head),
                ("origin/main", Kind::Remote),
                ("◆ v1", Kind::Tag)
            ]
        );
        assert_eq!(map[&oid('b')][0].kind, Kind::Branch);
    }

    #[test]
    fn a_detached_head_gets_its_own_badge() {
        let head = HeadInfo {
            branch: None,
            oid: Some(oid('c')),
            ..HeadInfo::default()
        };
        let map = build(&[], &[], &head);
        assert_eq!(map[&oid('c')][0].label, "HEAD");
    }
}
