//! Hosting account support for Bitbucket.

use gf_hosting::{HostingProvider, ProviderKind, TokenSetup};

#[derive(Debug)]
pub struct Bitbucket;

impl HostingProvider for Bitbucket {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Bitbucket
    }

    fn matches_host(&self, name: &str) -> bool {
        name.contains("bitbucket")
    }

    fn token_setup(&self, _protocol: &str, host: &str) -> TokenSetup {
        if host == "bitbucket.org" {
            TokenSetup {
                page: Some(
                    "https://id.atlassian.com/manage-profile/security/api-tokens".to_owned(),
                ),
                guidance: "Bitbucket Cloud: enter your Bitbucket username and use an API token with repository read/write as the password.",
            }
        } else {
            TokenSetup {
                page: None,
                guidance: "Bitbucket: use a personal HTTP access token from Manage account.",
            }
        }
    }
}
