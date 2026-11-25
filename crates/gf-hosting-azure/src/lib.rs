//! Hosting account support for Azure.

use gf_hosting::{HostingProvider, ProviderKind, TokenSetup};

#[derive(Debug)]
pub struct Azure;

impl HostingProvider for Azure {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Azure
    }

    fn matches_host(&self, name: &str) -> bool {
        name == "dev.azure.com" || name.ends_with(".visualstudio.com")
    }

    fn token_setup(&self, _protocol: &str, _host: &str) -> TokenSetup {
        TokenSetup {
            page: Some("https://dev.azure.com/_usersSettings/tokens".to_owned()),
            guidance: "Azure: use a token with Code (Read & write).",
        }
    }
}
