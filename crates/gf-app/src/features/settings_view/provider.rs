//! Which hosting service a git server is, guessed from its host name, and
//! where that service makes the personal access tokens git logs in with.

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
            Self::Other => "a personal access token, or your password",
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
