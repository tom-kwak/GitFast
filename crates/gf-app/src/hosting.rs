//! Hosting providers registered by the desktop application.

use gf_hosting::{ProviderKind, Providers};

pub static PROVIDERS: Providers<'static> = Providers::new(&[
    &gf_hosting_github::GitHub,
    &gf_hosting_gitlab::GitLab,
    &gf_hosting_bitbucket::Bitbucket,
    &gf_hosting_gitea::Gitea,
    &gf_hosting_azure::Azure,
]);

pub fn provider_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Other => "other",
        kind => kind.name(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn registered_providers_detect_cloud_and_custom_hosts() {
        for (host, kind) in [
            ("github.com", ProviderKind::GitHub),
            ("GitHub.example.com", ProviderKind::GitHub),
            ("gitlab.com", ProviderKind::GitLab),
            ("gitlab.local:8080", ProviderKind::GitLab),
            ("bitbucket.org", ProviderKind::Bitbucket),
            ("codeberg.org", ProviderKind::Gitea),
            ("dev.azure.com", ProviderKind::Azure),
            ("git.example.com", ProviderKind::Other),
            ("git.example.com:8443", ProviderKind::Other),
        ] {
            assert_eq!(PROVIDERS.guess(host), kind, "{host}");
        }
    }

    #[test]
    fn saved_provider_overrides_take_precedence_over_host_detection() {
        let services = BTreeMap::from([
            ("git.example.com".to_owned(), "gitlab".to_owned()),
            ("github.example.com".to_owned(), "gitea".to_owned()),
            ("odd.example.com".to_owned(), "nonsense".to_owned()),
        ]);
        for (host, kind) in [
            ("git.example.com", ProviderKind::GitLab),
            ("github.example.com", ProviderKind::Gitea),
            ("odd.example.com", ProviderKind::Other),
            ("gitlab.com", ProviderKind::GitLab),
        ] {
            assert_eq!(PROVIDERS.for_host(&services, host), kind, "{host}");
        }
        for kind in ProviderKind::ALL {
            assert_eq!(ProviderKind::from_key(kind.key()), Some(kind));
        }
    }

    #[test]
    fn selection_uses_each_providers_token_policy() {
        let page = |kind, scheme, host| {
            PROVIDERS
                .token_setup(kind, scheme, host)
                .and_then(|setup| setup.page)
        };
        assert_eq!(
            page(ProviderKind::GitLab, "http", "gitlab.local:8080").as_deref(),
            Some(
                "http://gitlab.local:8080/-/user_settings/personal_access_tokens?name=gf&scopes=read_repository,write_repository"
            ),
        );
        assert!(
            page(ProviderKind::GitHub, "https", "github.com")
                .is_some_and(|url| url.starts_with("https://github.com/settings/tokens/new"))
        );
        assert_eq!(
            page(ProviderKind::Gitea, "https", "gitea.local").as_deref(),
            Some("https://gitea.local/user/settings/applications"),
        );
        assert_eq!(
            page(ProviderKind::Azure, "https", "team.visualstudio.com").as_deref(),
            Some("https://dev.azure.com/_usersSettings/tokens"),
        );
        assert!(page(ProviderKind::Other, "https", "git.example.com").is_none());

        let cloud = PROVIDERS
            .token_setup(ProviderKind::Bitbucket, "https", "bitbucket.org")
            .expect("Bitbucket Cloud");
        let self_hosted = PROVIDERS
            .token_setup(ProviderKind::Bitbucket, "https", "bitbucket.example.com")
            .expect("Bitbucket Data Center");
        assert!(cloud.page.is_some());
        assert!(cloud.guidance.contains("API token"));
        assert!(self_hosted.page.is_none());
        assert!(self_hosted.guidance.contains("HTTP access token"));
    }
}
