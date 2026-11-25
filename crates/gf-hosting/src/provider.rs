//! Provider identity, host selection and token setup.

use std::collections::BTreeMap;
use std::fmt::Debug;

/// A hosting service with a stable key for per-host overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    GitHub,
    GitLab,
    Bitbucket,
    Gitea,
    Azure,
    Other,
}

impl ProviderKind {
    pub const ALL: [Self; 6] = [
        Self::GitHub,
        Self::GitLab,
        Self::Bitbucket,
        Self::Gitea,
        Self::Azure,
        Self::Other,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
            Self::Bitbucket => "bitbucket",
            Self::Gitea => "gitea",
            Self::Azure => "azure",
            Self::Other => "other",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Bitbucket => "Bitbucket",
            Self::Gitea => "Gitea",
            Self::Azure => "Azure",
            Self::Other => "server",
        }
    }
}

/// The provider's requirements for an HTTPS Git credential.
#[derive(Debug)]
pub struct TokenSetup {
    pub page: Option<String>,
    pub guidance: &'static str,
}

/// Provider-specific knowledge used when connecting an account.
pub trait HostingProvider: Debug + Sync {
    fn kind(&self) -> ProviderKind;

    /// Whether the lower-case name without its port suggests this provider.
    fn matches_host(&self, name: &str) -> bool;

    fn token_setup(&self, protocol: &str, host: &str) -> TokenSetup;
}

/// The providers registered by the application. Registration order breaks matching ties.
#[derive(Debug)]
pub struct Providers<'a> {
    entries: &'a [&'a dyn HostingProvider],
}

impl<'a> Providers<'a> {
    pub const fn new(entries: &'a [&'a dyn HostingProvider]) -> Self {
        Self { entries }
    }

    pub fn guess(&self, host: &str) -> ProviderKind {
        let name = host.split_once(':').map_or(host, |(name, _)| name);
        let name = name.to_ascii_lowercase();
        self.entries
            .iter()
            .find(|provider| provider.matches_host(&name))
            .map_or(ProviderKind::Other, |provider| provider.kind())
    }

    pub fn for_host(&self, overrides: &BTreeMap<String, String>, host: &str) -> ProviderKind {
        overrides
            .get(host)
            .and_then(|key| ProviderKind::from_key(key))
            .unwrap_or_else(|| self.guess(host))
    }

    pub fn token_setup(
        &self,
        kind: ProviderKind,
        protocol: &str,
        host: &str,
    ) -> Option<TokenSetup> {
        self.entries
            .iter()
            .find(|provider| provider.kind() == kind)
            .map(|provider| provider.token_setup(protocol, host))
    }
}
