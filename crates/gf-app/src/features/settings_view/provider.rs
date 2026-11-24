//! Which hosting service a git server is, guessed from its host name, and
//! where that service makes the personal access tokens git logs in with.
//! The guess can be overridden per host (settings `services`).

use std::collections::BTreeMap;

/// A hosting service, as far as logging in goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    GitHub,
    GitLab,
    Bitbucket,
    Gitea,
    Azure,
    /// Any other git server.
    Other,
}

impl Provider {
    /// In the order the log-in dialog offers them.
    pub const ALL: [Self; 6] = [
        Self::GitHub,
        Self::GitLab,
        Self::Bitbucket,
        Self::Gitea,
        Self::Azure,
        Self::Other,
    ];

    /// The service of `host`: the one picked for it in the settings
    /// (`services`), else guessed from its name.
    pub fn for_host(services: &BTreeMap<String, String>, host: &str) -> Self {
        services
            .get(host)
            .and_then(|key| Self::from_key(key))
            .unwrap_or_else(|| Self::of(host))
    }

    /// How it is saved in the settings.
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
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    /// Offered to pick: `other` for [`Provider::Other`].
    pub fn pick_label(self) -> &'static str {
        match self {
            Self::Other => "other",
            p => p.name(),
        }
    }

    /// Guessed from the host (`github.com`, `gitlab.example.com` …);
    /// self-hosted servers whose name says nothing are [`Provider::Other`].
    pub fn of(host: &str) -> Self {
        let host = host.to_ascii_lowercase();
        let name = host.split(':').next().unwrap_or_default();
        if name == "github.com" || name.ends_with(".github.com") || name.contains("github") {
            Self::GitHub
        } else if name.contains("gitlab") {
            Self::GitLab
        } else if name.contains("bitbucket") {
            Self::Bitbucket
        } else if name == "codeberg.org" || name.contains("gitea") || name.contains("forgejo") {
            Self::Gitea
        } else if name == "dev.azure.com" || name.ends_with(".visualstudio.com") {
            Self::Azure
        } else {
            Self::Other
        }
    }

    /// Shown before the host: `GitHub`, … and `server` for any other.
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

    /// The page that makes a token for git, with the scopes git needs
    /// already picked where the service allows it. `None` when unknown.
    pub fn token_page(self, protocol: &str, host: &str) -> Option<String> {
        let base = format!("{protocol}://{host}");
        match self {
            Self::GitHub => Some(format!(
                "{base}/settings/tokens/new?description=gf&scopes=repo,workflow"
            )),
            Self::GitLab => Some(format!(
                "{base}/-/user_settings/personal_access_tokens?name=gf&scopes=read_repository,write_repository"
            )),
            Self::Bitbucket => {
                Some("https://id.atlassian.com/manage-profile/security/api-tokens".into())
            }
            Self::Gitea => Some(format!("{base}/user/settings/applications")),
            Self::Azure => Some("https://dev.azure.com/_usersSettings/tokens".into()),
            Self::Other => None,
        }
    }

    /// What to make on [`Self::token_page`], said in the log-in dialog.
    pub fn token_hint(self) -> &'static str {
        match self {
            Self::GitHub => "a classic token with the `repo` (and `workflow`) scope",
            Self::GitLab => "a token with `read_repository` and `write_repository`",
            Self::Bitbucket => "an API token; the user name is your Atlassian e-mail",
            Self::Gitea => "an access token with repository read and write",
            Self::Azure => "a token with Code (Read & write)",
            Self::Other => "a personal access token (some servers take your password)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_are_told_by_their_host() {
        assert_eq!(Provider::of("github.com"), Provider::GitHub);
        assert_eq!(Provider::of("GitHub.example.com"), Provider::GitHub);
        assert_eq!(Provider::of("gitlab.com"), Provider::GitLab);
        assert_eq!(Provider::of("gitlab.local:8080"), Provider::GitLab);
        assert_eq!(Provider::of("bitbucket.org"), Provider::Bitbucket);
        assert_eq!(Provider::of("codeberg.org"), Provider::Gitea);
        assert_eq!(Provider::of("dev.azure.com"), Provider::Azure);
        assert_eq!(Provider::of("git.example.com"), Provider::Other);
        // The port is not part of the name.
        assert_eq!(Provider::of("git.example.com:8443"), Provider::Other);
    }

    #[test]
    fn a_picked_service_wins_over_the_name() {
        let mut services = BTreeMap::new();
        services.insert("git.example.com".to_owned(), "gitlab".to_owned());
        services.insert("github.example.com".to_owned(), "gitea".to_owned());
        services.insert("odd.example.com".to_owned(), "nonsense".to_owned());
        assert_eq!(
            Provider::for_host(&services, "git.example.com"),
            Provider::GitLab
        );
        assert_eq!(
            Provider::for_host(&services, "github.example.com"),
            Provider::Gitea,
            "even against a name that says otherwise"
        );
        assert_eq!(
            Provider::for_host(&services, "odd.example.com"),
            Provider::Other,
            "an unknown key falls back to the guess"
        );
        assert_eq!(
            Provider::for_host(&services, "gitlab.com"),
            Provider::GitLab
        );
        for p in Provider::ALL {
            assert_eq!(Provider::from_key(p.key()), Some(p));
        }
    }

    #[test]
    fn token_pages_point_at_the_same_server() {
        assert_eq!(
            Provider::GitLab
                .token_page("http", "gitlab.local:8080")
                .as_deref(),
            Some(
                "http://gitlab.local:8080/-/user_settings/personal_access_tokens?name=gf&scopes=read_repository,write_repository"
            )
        );
        assert!(
            Provider::GitHub
                .token_page("https", "github.com")
                .is_some_and(|u| u.starts_with("https://github.com/settings/tokens/new"))
        );
        assert_eq!(Provider::Other.token_page("https", "git.example.com"), None);
    }
}
