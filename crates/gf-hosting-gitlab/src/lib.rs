//! Hosting account support for GitLab.

use gf_hosting::{HostingProvider, ProviderKind, TokenSetup};

#[derive(Debug)]
pub struct GitLab;

impl HostingProvider for GitLab {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GitLab
    }

    fn matches_host(&self, name: &str) -> bool {
        name.contains("gitlab")
    }

    fn token_setup(&self, protocol: &str, host: &str) -> TokenSetup {
        TokenSetup {
            page: Some(format!(
                "{protocol}://{host}/-/user_settings/personal_access_tokens?name=gf&scopes=read_repository,write_repository"
            )),
            guidance: "GitLab: use a token with `read_repository` and `write_repository`.",
        }
    }
}
