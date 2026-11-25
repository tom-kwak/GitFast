//! Hosting account support for Gitea.

use gf_hosting::{HostingProvider, ProviderKind, TokenSetup};

#[derive(Debug)]
pub struct Gitea;

impl HostingProvider for Gitea {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Gitea
    }

    fn matches_host(&self, name: &str) -> bool {
        name == "codeberg.org" || name.contains("gitea") || name.contains("forgejo")
    }

    fn token_setup(&self, protocol: &str, host: &str) -> TokenSetup {
        TokenSetup {
            page: Some(format!("{protocol}://{host}/user/settings/applications")),
            guidance: "Gitea: use an access token with repository read and write.",
        }
    }
}
