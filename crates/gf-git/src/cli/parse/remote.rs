//! `git remote -v`.

use gf_core::model::Remote;
use gf_core::{GfError, Result};

pub const REMOTE_ARGS: &[&str] = &["remote", "-v"];

/// Parses lines like `origin\tgit@host:a/b.git (fetch)`, preserving the
/// order in which remotes first appear.
pub fn parse_remotes(out: &[u8]) -> Result<Vec<Remote>> {
    let mut remotes: Vec<Remote> = Vec::new();
    for line in String::from_utf8_lossy(out)
        .lines()
        .filter(|l| !l.is_empty())
    {
        let bad = || GfError::parse("remote -v", line.to_owned());
        let (name, rest) = line.split_once('\t').ok_or_else(bad)?;
        let (url, kind) = rest.rsplit_once(' ').ok_or_else(bad)?;

        let idx = match remotes.iter().position(|r| r.name == name) {
            Some(i) => i,
            None => {
                remotes.push(Remote {
                    name: name.to_owned(),
                    fetch_refspecs: Vec::new(),
                    fetch_url: String::new(),
                    push_url: None,
                });
                remotes.len() - 1
            }
        };
        let remote = &mut remotes[idx];
        match kind {
            "(fetch)" => remote.fetch_url = url.to_owned(),
            "(push)" => remote.push_url = Some(url.to_owned()),
            _ => return Err(bad()),
        }
    }
    // Only keep a push URL when it differs from the fetch URL.
    for r in &mut remotes {
        if r.push_url.as_deref() == Some(r.fetch_url.as_str()) {
            r.push_url = None;
        }
    }
    Ok(remotes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remotes() {
        let out = b"origin\tgit@gitlab.com:team/app.git (fetch)\n\
                    origin\tgit@gitlab.com:team/app.git (push)\n\
                    fork\thttps://example.com/me/app.git (fetch)\n\
                    fork\tssh://push.example.com/me/app.git (push)\n";
        let remotes = parse_remotes(out).expect("parse");
        assert_eq!(remotes.len(), 2);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].push_url, None);
        assert_eq!(
            remotes[1].effective_push_url(),
            "ssh://push.example.com/me/app.git"
        );
    }

    #[test]
    fn rejects_malformed_lines() {
        assert!(parse_remotes(b"origin url-without-tab (fetch)\n").is_err());
    }
}
