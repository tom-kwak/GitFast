use gf_core::Result;
use gf_core::model::Remote;

use crate::backend::{CliBackend, RemoteOps};
use crate::cli::parse::remote::{REMOTE_ARGS, parse_remotes};

impl RemoteOps for CliBackend {
    async fn remotes(&self) -> Result<Vec<Remote>> {
        let mut remotes = parse_remotes(&self.read(self.command(REMOTE_ARGS)).await?)?;
        if !remotes.is_empty() {
            let cmd = self
                .command(["config", "-z", "--get-regexp", r"^remote\..*\.fetch$"])
                .read_only()
                .allow_failure();
            let output = self.runner().run(&cmd).await?;
            if !matches!(output.exit_code, Some(0 | 1)) {
                return Err(gf_core::GfError::Other(output.stderr));
            }
            for entry in output.stdout_str().split_terminator('\0') {
                if let Some((key, value)) = entry.split_once('\n')
                    && let Some(name) = key
                        .strip_prefix("remote.")
                        .and_then(|s| s.strip_suffix(".fetch"))
                    && let Some(remote) = remotes.iter_mut().find(|r| r.name == name)
                {
                    remote.fetch_refspecs.push(value.to_owned());
                }
            }
        }
        Ok(remotes)
    }
}

/// Resolve a tracking ref back to a unique remote head using the configured fetch mappings.
pub fn remote_branch<'a>(remotes: &'a [Remote], tracking: &str) -> Option<(&'a str, String)> {
    use gix_refspec::{MatchGroup, match_group::Item, parse::Operation};
    let target = gix_hash::Kind::Sha1.null();
    let item = Item {
        full_ref_name: tracking.into(),
        target: &target,
        object: None,
    };
    let mut found = None;
    for remote in remotes {
        let specs = remote
            .fetch_refspecs
            .iter()
            .map(|spec| gix_refspec::parse(spec.as_str().into(), Operation::Fetch))
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()?;
        let group = MatchGroup::from_fetch_specs(specs);
        for mapping in group.match_rhs(std::iter::once(item)).mappings {
            let source = mapping.lhs.to_string();
            let head = source.strip_prefix("refs/heads/")?;
            if found.is_some() {
                return None;
            }
            found = Some((remote.name.as_str(), head.to_owned()));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unique_included_remote_heads_can_be_deleted() {
        let remote = |name: &str, specs: &[&str]| Remote {
            name: name.to_owned(),
            fetch_url: String::new(),
            push_url: None,
            fetch_refspecs: specs.iter().map(|s| (*s).to_owned()).collect(),
        };
        let team = remote(
            "team/origin",
            &[
                "+refs/heads/release/*:refs/remotes/review/*",
                "^refs/heads/release/private",
                "refs/tags/v1:refs/remotes/review-tag",
            ],
        );
        let remotes = std::slice::from_ref(&team);
        assert_eq!(
            remote_branch(remotes, "refs/remotes/review/v1"),
            Some(("team/origin", "release/v1".to_owned()))
        );
        assert_eq!(remote_branch(remotes, "refs/remotes/review/private"), None);
        assert_eq!(remote_branch(remotes, "refs/remotes/review-tag"), None);
        let other = remote("other", &["refs/heads/*:refs/remotes/review/*"]);
        assert_eq!(
            remote_branch(&[team, other], "refs/remotes/review/v1"),
            None
        );
    }
}
