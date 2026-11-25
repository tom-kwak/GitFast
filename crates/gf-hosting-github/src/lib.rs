//! Hosting account support for GitHub.

use gf_hosting::{HostingProvider, ProviderKind, TokenSetup};

#[derive(Debug)]
pub struct GitHub;

impl HostingProvider for GitHub {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GitHub
    }

    fn matches_host(&self, name: &str) -> bool {
        name.contains("github")
    }

    fn token_setup(&self, protocol: &str, host: &str) -> TokenSetup {
        TokenSetup {
            page: Some(format!(
                "{protocol}://{host}/settings/tokens/new?description=gf&scopes=repo,workflow"
            )),
            guidance: "GitHub: use a classic token with the `repo` (and `workflow`) scope.",
        }
    }
}
