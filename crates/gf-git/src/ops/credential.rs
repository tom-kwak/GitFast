//! Logins to git servers, kept by git's own credential helper.
//!
//! gf does not store tokens itself: `git credential approve` hands them to
//! whatever helper the user's git is set up with (`osxkeychain`, Git
//! Credential Manager, `store` …). So a login made in gf also works for git
//! in the terminal, and the other way around.

use std::fmt;

use gf_core::{GfError, Result};

use crate::cli::{GitCommand, GitRunner};

/// A server and who to log in as. The token never shows in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Login {
    /// `https` or `http`.
    pub protocol: String,
    /// Host name, with `:port` when not the default one.
    pub host: String,
    pub username: String,
    /// Password or personal access token.
    pub token: String,
}

impl fmt::Debug for Login {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Login")
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("username", &self.username)
            .field("token", &"…")
            .finish()
    }
}

/// The protocol and host of an `http(s)://` URL, or of a bare host
/// (`github.com`, taken as https). `None` for SSH and other URLs, which do
/// not log in with a token. A user name in the URL and any path are
/// dropped: `https://me@git.example.com:8443/a/b.git` →
/// `("https", "git.example.com:8443")`.
pub fn server_of(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (protocol, rest) = match url.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            if scheme != "https" && scheme != "http" {
                return None;
            }
            (scheme, rest)
        }
        // `git@host:path` is SSH.
        None if url.contains('@') || (url.contains(':') && !has_port(url)) => return None,
        None => ("https".to_owned(), url),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let valid = host.chars().any(|c| c.is_ascii_alphanumeric())
        && !host.starts_with(['.', '-', ':'])
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));
    valid.then(|| (protocol, host.to_ascii_lowercase()))
}

/// `host:8443` (a port), as opposed to `host:path` (SSH).
fn has_port(url: &str) -> bool {
    let authority = url.split('/').next().unwrap_or_default();
    authority
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
}

/// What `git credential` reads on stdin for this login.
fn credential_input(protocol: &str, host: &str, username: &str, token: Option<&str>) -> String {
    let mut input = format!("protocol={protocol}\nhost={host}\nusername={username}\n");
    if let Some(token) = token {
        input.push_str(&format!("password={token}\n"));
    }
    input.push('\n');
    input
}

fn check_value(what: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(GfError::Other(format!("{what} is empty")));
    }
    if value.contains(['\n', '\r', '\0']) {
        return Err(GfError::Other(format!("{what} cannot contain line breaks")));
    }
    Ok(())
}

/// Runs outside any repository, so a repository's own config never
/// changes where the login goes.
fn global(args: &[&str]) -> GitCommand {
    GitCommand::new(args).current_dir(std::env::temp_dir())
}

/// The credential helper git would use for this server (the last one set,
/// as `git config --get-urlmatch` reports it); `None` when there is none,
/// and git would forget a login as soon as it was made.
pub async fn credential_helper(
    runner: &GitRunner,
    protocol: &str,
    host: &str,
) -> Result<Option<String>> {
    let url = format!("{protocol}://{host}/");
    let out = runner
        .run(
            &global(&["config", "--get-urlmatch", "credential.helper", &url])
                .read_only()
                .allow_failure(),
        )
        .await?;
    // Exit code 1: not set. Anything else non-zero is a real error.
    match out.exit_code {
        Some(0) => {
            let helper = out.stdout_str().trim().to_owned();
            Ok((!helper.is_empty()).then_some(helper))
        }
        Some(1) => Ok(None),
        code => Err(GfError::GitCommand {
            argv: vec!["config".into(), "--get-urlmatch".into()],
            exit_code: code,
            stderr: out.stderr.trim().to_owned(),
        }),
    }
}

/// Saves the login with git's credential helper (`git credential
/// approve`). Fails when git has no helper set, since nothing would keep it.
/// Returns the helper's name.
pub async fn store_login(runner: &GitRunner, login: &Login) -> Result<String> {
    check_login_values(login)?;
    let Some(helper) = credential_helper(runner, &login.protocol, &login.host).await? else {
        return Err(GfError::Other(NO_HELPER.into()));
    };
    let input = credential_input(
        &login.protocol,
        &login.host,
        &login.username,
        Some(&login.token),
    );
    runner
        .run(&global(&["credential", "approve"]).stdin(input))
        .await?;
    Ok(helper)
}

fn check_login_values(login: &Login) -> Result<()> {
    check_value("the server", &login.host)?;
    check_value("the user name", &login.username)?;
    check_value("the token", &login.token)
}

/// Said when git has no credential helper to keep a login in.
pub const NO_HELPER: &str = "git has no credential helper to keep the login in. \
     Set one first, e.g. `git config --global credential.helper osxkeychain` (macOS) \
     or `manager` (Windows).";

/// Removes the saved login from git's credential helper (`git credential
/// reject`). Succeeds also when nothing was saved.
pub async fn forget_login(
    runner: &GitRunner,
    protocol: &str,
    host: &str,
    username: &str,
) -> Result<()> {
    check_value("the server", host)?;
    check_value("the user name", username)?;
    runner
        .run(
            &global(&["credential", "reject"])
                .stdin(credential_input(protocol, host, username, None)),
        )
        .await?;
    Ok(())
}

/// Asks git without ever prompting: no terminal (the runner's
/// `GIT_TERMINAL_PROMPT=0`), no askpass program, no helper window.
fn quiet(cmd: GitCommand) -> GitCommand {
    cmd.env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("GCM_INTERACTIVE", "never")
}

/// Whether git's credential helper still has a token for this login.
/// git erases one the server refused, so `false` usually means it expired
/// or was revoked (or was removed by hand).
pub async fn has_login(
    runner: &GitRunner,
    protocol: &str,
    host: &str,
    username: &str,
) -> Result<bool> {
    Ok(saved_login(runner, protocol, host, username)
        .await?
        .is_some())
}

/// Retrieves a matching saved login without prompting or logging the token.
pub async fn saved_login(
    runner: &GitRunner,
    protocol: &str,
    host: &str,
    username: &str,
) -> Result<Option<Login>> {
    check_value("the server", host)?;
    check_value("the user name", username)?;
    let out = runner
        .run(
            &quiet(global(&[
                "-c",
                "credential.interactive=false",
                "credential",
                "fill",
            ]))
            .stdin(credential_input(protocol, host, username, None))
            .read_only()
            .allow_failure(),
        )
        .await?;
    let stdout = out.stdout_str();
    let matched = out.success()
        && stdout
            .lines()
            .any(|line| line.strip_prefix("username=") == Some(username));
    if !matched {
        return Ok(None);
    }
    Ok(stdout
        .lines()
        .filter_map(|line| line.strip_prefix("password="))
        .find(|password| !password.is_empty())
        .map(|token| Login {
            protocol: protocol.to_owned(),
            host: host.to_owned(),
            username: username.to_owned(),
            token: token.to_owned(),
        }))
}

// Git invokes shell credential helpers with an operation argument. Supply
// the candidate only to its own origin, even after a redirect or URL rewrite.
const CANDIDATE_HELPER: &str = r#"!f() {
    case "$1" in get|store) ;; *) return 0 ;; esac
    protocol= host=
    while IFS= read -r line && [ -n "$line" ]; do
        case "$line" in
            protocol=*) protocol=${line#protocol=} ;;
            host=*) host=${line#host=} ;;
        esac
    done
    if [ "$protocol" = "$GF_LOGIN_PROTOCOL" ] &&
        [ "$(printf '%s' "$host" | LC_ALL=C tr 'A-Z' 'a-z')" = "$GF_LOGIN_HOST" ]; then
        if [ "$1" = get ]; then
            printf 'username=%s\npassword=%s\n\n' "$GF_LOGIN_USERNAME" "$GF_LOGIN_TOKEN"
        else
            : > "$GF_LOGIN_ACCEPTED"
        fi
    fi
}; f"#;

/// Checks read access to a remote with only the candidate login available.
/// Returns whether Git approved the candidate after authentication. A
/// public remote can succeed anonymously. The check leaves the user's
/// configured helpers and saved logins untouched.
pub async fn check_remote_access(runner: &GitRunner, url: &str, login: &Login) -> Result<bool> {
    check_login_values(login)?;
    let marker_dir = tempfile::tempdir()?;
    let accepted = marker_dir.path().join("accepted");
    runner
        .run(
            &quiet(global(&[
                "-c",
                "credential.interactive=true",
                "-c",
                "credential.helper=",
                "--config-env=credential.helper=GF_LOGIN_HELPER",
                "ls-remote",
                "--heads",
                "--",
                url,
            ]))
            .env("GF_LOGIN_PROTOCOL", &login.protocol)
            .env("GF_LOGIN_HELPER", CANDIDATE_HELPER)
            .env("GF_LOGIN_HOST", &login.host)
            .env("GF_LOGIN_USERNAME", &login.username)
            .env("GF_LOGIN_TOKEN", &login.token)
            .env("GF_LOGIN_ACCEPTED", &accepted)
            .read_only(),
        )
        .await?;
    Ok(tokio::fs::try_exists(accepted).await?)
}

/// Sets git's credential helper for every repository of this user
/// (`git config --global credential.helper <helper>`).
pub async fn set_credential_helper(runner: &GitRunner, helper: &str) -> Result<()> {
    check_value("the helper", helper)?;
    runner
        .run(&global(&[
            "config",
            "--global",
            "credential.helper",
            helper,
        ]))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_come_from_https_urls_and_bare_hosts() {
        let s = |p: &str, h: &str| Some((p.to_owned(), h.to_owned()));
        assert_eq!(
            server_of("https://github.com/a/b.git"),
            s("https", "github.com")
        );
        assert_eq!(
            server_of("https://me@Git.Example.com:8443/a"),
            s("https", "git.example.com:8443")
        );
        assert_eq!(server_of("http://10.0.0.5/x"), s("http", "10.0.0.5"));
        assert_eq!(server_of(" github.com "), s("https", "github.com"));
        assert_eq!(
            server_of("gitlab.local:8080"),
            s("https", "gitlab.local:8080")
        );
        assert_eq!(server_of("github.com/a/b"), s("https", "github.com"));
    }

    #[test]
    fn ssh_and_other_urls_have_no_login() {
        for url in [
            "git@github.com:a/b.git",
            "ssh://git@github.com/a/b.git",
            "github.com:a/b.git",
            "file:///tmp/repo",
            "/tmp/repo",
            "",
            "https://",
            "https://bad host/",
            ".",
            "..",
        ] {
            assert_eq!(server_of(url), None, "{url}");
        }
    }

    #[test]
    fn the_token_stays_out_of_debug_output() {
        let login = Login {
            protocol: "https".into(),
            host: "github.com".into(),
            username: "me".into(),
            token: "ghp_secret".into(),
        };
        assert!(!format!("{login:?}").contains("ghp_secret"));
    }

    #[test]
    fn credential_input_ends_with_a_blank_line() {
        assert_eq!(
            credential_input("https", "h", "u", Some("t")),
            "protocol=https\nhost=h\nusername=u\npassword=t\n\n"
        );
        assert_eq!(
            credential_input("https", "h", "u", None),
            "protocol=https\nhost=h\nusername=u\n\n"
        );
    }
}
