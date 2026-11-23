#![allow(clippy::unwrap_used)]

//! Logins go to git's credential helper. Each test points git at its own
//! global config and a `store` helper file, so the user's keychain is never
//! touched.

use std::path::{Path, PathBuf};

use gf_git::GitRunner;
use gf_git::ops::credential::{Login, NO_HELPER, credential_helper, forget_login, store_login};

struct Home {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Home {
    /// `helper`: the global `credential.helper`, if any.
    fn new(helper: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let mut config = String::new();
        if let Some(helper) = helper {
            let helper = helper.replace("{store}", &slash(&root.join("store")));
            config.push_str(&format!("[credential]\n\thelper = {helper}\n"));
        }
        std::fs::write(root.join("gitconfig"), config).unwrap();
        Self { _dir: dir, root }
    }

    fn runner(&self) -> GitRunner {
        GitRunner::new().with_cleared_env().with_env([
            ("PATH", std::env::var_os("PATH").unwrap_or_default()),
            ("HOME", self.root.clone().into_os_string()),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
            (
                "GIT_CONFIG_GLOBAL",
                self.root.join("gitconfig").into_os_string(),
            ),
            ("GIT_TERMINAL_PROMPT", "0".into()),
        ])
    }

    fn stored(&self) -> String {
        std::fs::read_to_string(self.root.join("store")).unwrap_or_default()
    }
}

/// Forward slashes: git config values treat `\` as an escape.
fn slash(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

fn login(host: &str, user: &str, token: &str) -> Login {
    Login {
        protocol: "https".into(),
        host: host.into(),
        username: user.into(),
        token: token.into(),
    }
}

#[tokio::test]
async fn a_login_is_kept_by_the_credential_helper_and_forgotten_on_logout() {
    let home = Home::new(Some("store --file={store}"));
    let runner = home.runner();

    let helper = store_login(&runner, &login("git.example.com:8443", "me", "tok-1"))
        .await
        .unwrap();
    assert!(helper.starts_with("store"), "{helper}");
    assert_eq!(
        home.stored().trim(),
        "https://me:tok-1@git.example.com%3a8443"
    );

    store_login(&runner, &login("github.com", "you", "tok-2"))
        .await
        .unwrap();
    assert!(home.stored().contains("https://you:tok-2@github.com"));

    forget_login(&runner, "https", "git.example.com:8443", "me")
        .await
        .unwrap();
    assert!(!home.stored().contains("tok-1"));
    assert!(home.stored().contains("tok-2"));

    // Nothing saved for it: still fine.
    forget_login(&runner, "https", "nowhere.example", "me")
        .await
        .unwrap();
}

#[tokio::test]
async fn without_a_credential_helper_the_login_is_refused() {
    let home = Home::new(None);
    let runner = home.runner();
    assert_eq!(
        credential_helper(&runner, "https", "github.com")
            .await
            .unwrap(),
        None
    );
    let err = store_login(&runner, &login("github.com", "me", "tok"))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), NO_HELPER);
}

#[tokio::test]
async fn a_helper_set_for_one_server_counts_only_there() {
    let home = Home::new(None);
    std::fs::write(
        home.root.join("gitconfig"),
        "[credential \"https://git.example.com\"]\n\thelper = cache\n",
    )
    .unwrap();
    let runner = home.runner();
    assert_eq!(
        credential_helper(&runner, "https", "git.example.com")
            .await
            .unwrap()
            .as_deref(),
        Some("cache")
    );
    assert_eq!(
        credential_helper(&runner, "https", "github.com")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn line_breaks_never_reach_git() {
    let home = Home::new(Some("store --file={store}"));
    let runner = home.runner();
    let bad = login("github.com", "me\nhost=evil.example", "tok");
    assert!(store_login(&runner, &bad).await.is_err());
    assert!(
        store_login(&runner, &login("github.com", "me", ""))
            .await
            .is_err()
    );
    assert_eq!(home.stored(), "");
}
