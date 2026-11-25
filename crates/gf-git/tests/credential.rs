#![allow(clippy::unwrap_used)]

//! Logins go to git's credential helper. Each test points git at its own
//! global config and a `store` helper file, so the user's keychain is never
//! touched.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gf_git::GitRunner;
use gf_git::ops::credential::{
    Login, NO_HELPER, check_remote_access, credential_helper, forget_login, has_login,
    set_credential_helper, store_login,
};

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

#[tokio::test]
async fn a_saved_token_is_found_until_it_is_forgotten() {
    let home = Home::new(Some("store --file={store}"));
    let runner = home.runner();
    assert!(
        !has_login(&runner, "https", "github.com", "me")
            .await
            .unwrap()
    );
    store_login(&runner, &login("github.com", "me", "tok"))
        .await
        .unwrap();
    assert!(
        has_login(&runner, "https", "github.com", "me")
            .await
            .unwrap()
    );
    // Someone else on the same server has none.
    assert!(
        !has_login(&runner, "https", "github.com", "you")
            .await
            .unwrap()
    );
    forget_login(&runner, "https", "github.com", "me")
        .await
        .unwrap();
    assert!(
        !has_login(&runner, "https", "github.com", "me")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn checking_a_rejected_replacement_preserves_the_saved_token() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let home = Home::new(Some("store --file={store}"));
    let runner = home.runner();
    let mut candidate = login(&host, "me", "old");
    candidate.protocol = "http".into();
    store_login(&runner, &candidate).await.unwrap();
    let before = home.stored();

    let (requests, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut client, _) = listener.accept().unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let headers: Vec<_> = BufReader::new(&client)
                .lines()
                .map(|line| line.unwrap())
                .take_while(|line| !line.is_empty())
                .collect();
            requests.send(headers).unwrap();
            client
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"test\"\r\n\
                      Content-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        }
    });

    candidate.token = "new".into();
    let url = format!("http://{host}/private.git");
    let error = tokio::time::timeout(
        Duration::from_secs(10),
        check_remote_access(&runner, &url, &candidate),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(
        error.to_string().contains("Authentication failed"),
        "{error}"
    );
    let first = received.recv_timeout(Duration::from_secs(1)).unwrap();
    let second = received.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(!first.iter().any(|line| line.starts_with("Authorization:")));
    assert!(
        second
            .iter()
            .any(|line| line == "Authorization: Basic bWU6bmV3")
    );
    assert_eq!(home.stored(), before);
}

#[tokio::test]
async fn only_a_server_that_accepts_the_token_verifies_the_login() {
    let home = Home::new(Some("store --file={store}"));
    let runner = home.runner();
    for requires_login in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let (requests, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for challenge in [true, false].into_iter().skip(usize::from(!requires_login)) {
                let (mut client, _) = listener.accept().unwrap();
                client
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let authenticated = BufReader::new(&client)
                    .lines()
                    .map(|line| line.unwrap())
                    .take_while(|line| !line.is_empty())
                    .any(|line| line == "Authorization: Basic bWU6dG9r");
                requests.send(authenticated).unwrap();
                if challenge {
                    client.write_all(
                        b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"test\"\r\n\
                          Content-Length: 0\r\nConnection: close\r\n\r\n"
                    ).unwrap();
                } else {
                    let advertisement = "001e# service=git-upload-pack\n00000000";
                    write!(client,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/x-git-upload-pack-advertisement\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{advertisement}",
                        advertisement.len()
                    ).unwrap();
                }
            }
        });
        let mut login = login(&host, "me", "tok");
        login.protocol = "http".into();
        let url = format!("http://{host}/repo.git");
        let verified = tokio::time::timeout(
            Duration::from_secs(10),
            check_remote_access(&runner, &url, &login),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!received.recv_timeout(Duration::from_secs(1)).unwrap());
        if requires_login {
            assert!(received.recv_timeout(Duration::from_secs(1)).unwrap());
        }
        assert_eq!(verified, requires_login);
        assert!(
            home.stored().is_empty(),
            "a check does not persist credentials"
        );
    }
}

#[tokio::test]
async fn without_a_helper_no_token_is_found_and_none_is_asked_for() {
    let home = Home::new(None);
    assert!(
        !has_login(&home.runner(), "https", "github.com", "me")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn a_helper_returning_a_different_username_does_not_count_as_saved() {
    let home = Home::new(Some(r#"!echo username=other && echo password=token"#));
    assert!(
        !has_login(&home.runner(), "https", "github.com", "me")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn the_credential_helper_can_be_set_for_the_user() {
    let home = Home::new(None);
    let runner = home.runner();
    set_credential_helper(&runner, "cache").await.unwrap();
    assert_eq!(
        credential_helper(&runner, "https", "github.com")
            .await
            .unwrap()
            .as_deref(),
        Some("cache")
    );
}
