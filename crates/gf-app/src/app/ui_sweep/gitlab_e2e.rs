//! Against a real GitLab (`--ignored`, and only when `GF_GITLAB_URL` is
//! set): log in, clone, push, log out, fetch, cancel a clone. Credentials
//! go to a `store` helper file in the test's home, never the keychain.
//!
//! ```text
//! GF_GITLAB_URL=http://localhost:8929 GF_GITLAB_USER=root GF_GITLAB_TOKEN=glpat-… \
//!   cargo test -p gf-app gitlab_e2e -- --ignored
//! ```
//!
//! Needs private projects `<user>/gf-demo` (any content) and `<user>/gf-big`
//! (large enough that a clone takes a moment, ~150 MB).

use iced_test::runtime::{self, Action};

use super::*;

struct Server {
    url: String,
    user: String,
    token: String,
}

fn server() -> Option<Server> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    Some(Server {
        url: var("GF_GITLAB_URL")?.trim_end_matches('/').to_owned(),
        user: var("GF_GITLAB_USER").unwrap_or_else(|| "root".into()),
        token: var("GF_GITLAB_TOKEN")?,
    })
}

impl Server {
    fn repo(&self, name: &str) -> String {
        format!("{}/{}/{name}.git", self.url, self.user)
    }

    /// How the login dialog writes the server.
    fn label(&self) -> String {
        let (protocol, host) = gf_git::ops::credential::server_of(&self.url).expect("server url");
        if protocol == "https" {
            host
        } else {
            format!("{protocol}://{host}")
        }
    }
}

fn login_dialog(ui: &Ui) -> &Dialog<Purpose> {
    let dialog = ui.app.dialog.as_ref().expect("a dialog");
    assert_eq!(dialog.title, "Log in to a git server", "{:?}", dialog.body);
    dialog
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=gf", "-c", "user.email=gf@example.com"])
        .args(args)
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[tokio::test]
#[ignore = "needs a GitLab server (GF_GITLAB_URL)"]
async fn gitlab_login_clone_push_logout_fetch() {
    let Some(server) = server() else {
        eprintln!("GF_GITLAB_URL not set: skipped");
        return;
    };
    let mut ui = Ui::welcome().await;
    let store = with_store_helper(&mut ui);
    let stored = || std::fs::read_to_string(&store).unwrap_or_default();
    let dest = ui._home.path().join("work/gf-demo");

    // Open window: the URL offers a clone, which asks where to.
    let _ = ui.send(Message::OpenPicker).await;
    let _ = ui
        .send(Message::Picker(repo_picker::Message::Filter(
            server.repo("gf-demo"),
        )))
        .await;
    let _ = ui
        .send(Message::Picker(repo_picker::Message::Pick(0)))
        .await;
    let _ = ui
        .send(Message::Dialog(dialog::Message::Field(
            1,
            dest.display().to_string(),
        )))
        .await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;

    // A private project: the server wants a login.
    let dialog = login_dialog(&ui);
    assert_eq!(dialog.fields[0].value, server.label());
    assert_eq!(dialog.confirm_label, "Log in and clone gf-demo");
    assert!(!dest.exists(), "the failed clone left nothing");

    // A wrong token: git drops it and the dialog comes back.
    fill_login(&mut ui, &server.label(), &server.user, "glpat-wrong").await;
    let dialog = login_dialog(&ui);
    assert_eq!(dialog.fields[1].value, server.user, "the user name is kept");
    assert!(
        !stored().contains("glpat-wrong"),
        "git forgot the refused token"
    );

    // The right one: cloned and opened.
    fill_login(&mut ui, &server.label(), &server.user, &server.token).await;
    assert!(
        ui.app.dialog.is_none(),
        "{:?}",
        ui.app
            .dialog
            .as_ref()
            .map(|d| (&d.title, &d.error, &d.body))
    );
    assert!(stored().contains(&server.token), "kept by the helper");
    assert_eq!(ui.app.tabs.len(), 1, "opened");
    assert_eq!(ui.app.settings.accounts.len(), 1);

    // Push a commit with the saved login.
    std::fs::write(dest.join("from-gf.txt"), format!("{:?}\n", Instant::now())).expect("write");
    let _ = git(&dest, &["add", "from-gf.txt"]);
    let _ = git(&dest, &["commit", "-qm", "from gf e2e"]);
    let head = git(&dest, &["rev-parse", "HEAD"]);
    let _ = ui.send(Message::Toolbar(toolbar::Message::Refresh)).await;
    let _ = ui.send(Message::Toolbar(toolbar::Message::Push)).await;
    assert!(
        ui.app.dialog.is_none(),
        "{:?}",
        ui.app.dialog.as_ref().map(|d| (&d.title, &d.body))
    );
    // Read back with gf's git (and its saved login).
    let remote = ui
        .app
        .runner
        .run(
            &gf_git::GitCommand::new(["ls-remote", "origin", "refs/heads/main"]).current_dir(&dest),
        )
        .await
        .expect("ls-remote")
        .stdout_str();
    assert!(remote.starts_with(&head), "pushed: {remote}");

    // Log out: the helper forgets the token, and the next fetch asks.
    let _ = ui.send(Message::Key(Shortcut::Settings)).await;
    let _ = ui
        .send(Message::Settings(settings_view::Message::LogOut(0)))
        .await;
    let _ = ui.send(Message::Dialog(dialog::Message::Submit)).await;
    assert!(ui.app.settings.accounts.is_empty());
    assert!(!stored().contains(&server.token), "forgotten");

    // The open clone's server is offered; a wrong token is tried on it
    // and refused before it is kept.
    assert!(ui.shows("─ NOT LOGGED IN"));
    let _ = ui
        .send(Message::Settings(settings_view::Message::LogInTo(
            server.label(),
        )))
        .await;
    fill_login(&mut ui, &server.label(), &server.user, "glpat-wrong-token").await;
    let dialog = login_dialog(&ui);
    assert!(
        dialog
            .error
            .as_deref()
            .is_some_and(|e| e.contains("refused")),
        "{:?}",
        dialog.error
    );
    assert!(!stored().contains("glpat-wrong-token"), "not kept");
    assert!(ui.app.settings.accounts.is_empty());
    let _ = ui.send(Message::Key(Shortcut::Escape)).await;
    assert!(ui.app.dialog.is_none());

    let _ = ui.send(Message::Key(Shortcut::Settings)).await;
    assert!(!ui.app.settings_open);

    let _ = ui.send(Message::Toolbar(toolbar::Message::Fetch)).await;
    let dialog = login_dialog(&ui);
    assert_eq!(dialog.confirm_label, "Log in and fetch all remotes");
    fill_login(&mut ui, &server.label(), &server.user, &server.token).await;
    assert!(ui.app.dialog.is_none());
    assert!(
        ui.flash().contains("fetch") || ui.flash().contains("logged in"),
        "{}",
        ui.flash()
    );
}

#[tokio::test]
#[ignore = "needs a GitLab server (GF_GITLAB_URL)"]
async fn gitlab_clone_shows_progress_and_cancels() {
    let Some(server) = server() else {
        eprintln!("GF_GITLAB_URL not set: skipped");
        return;
    };
    let mut ui = Ui::welcome().await;
    let store = with_store_helper(&mut ui);
    // Logged in already (as from the settings).
    std::fs::write(
        &store,
        format!(
            "{}://{}:{}@{}\n",
            server.url.split("://").next().expect("scheme"),
            server.user,
            server.token,
            server.url.split("://").nth(1).expect("host")
        ),
    )
    .expect("store");
    let dest = ui._home.path().join("big");
    let folder = dest.display().to_string();

    // Drive the clone by hand: cancel at the first progress report.
    let task = ui.app.run_clone(&server.repo("gf-big"), &folder);
    let mut stream = runtime::task::into_stream(task).expect("a running clone");
    let mut progress = Vec::new();
    let done = loop {
        let next = tokio::time::timeout(Duration::from_secs(60), stream.next())
            .await
            .expect("the clone reports in time");
        match next {
            Some(Action::Output(Message::CloneProgress(p))) => {
                assert!(ui.shows("[ cancel Esc ]") || progress.is_empty());
                let _ = ui.app.clone_progress(p.clone());
                if progress.is_empty() {
                    assert!(
                        ui.app
                            .cloning
                            .as_ref()
                            .is_some_and(|c| c.status().contains('%'))
                    );
                    let _ = ui.send(Message::Key(Shortcut::Escape)).await;
                }
                progress.push(p);
            }
            Some(Action::Output(Message::CloneDone(done))) => break done,
            Some(_) => {}
            None => panic!("ended without CloneDone"),
        }
    };
    assert!(!progress.is_empty(), "progress was reported");
    assert!(
        done.result.as_ref().is_err_and(|e| e.contains("cancelled")),
        "{:?}",
        done.result
    );
    let _ = ui.send(Message::CloneDone(done)).await;
    assert!(ui.app.cloning.is_none());
    assert!(ui.flash().contains("cancelled"), "{}", ui.flash());
    assert!(!dest.exists(), "the partial clone is gone");
    assert!(ui.app.tabs.is_empty());

    // And without cancelling, it finishes and opens.
    let task = ui.app.run_clone(&server.repo("gf-big"), &folder);
    let mut stream = runtime::task::into_stream(task).expect("a running clone");
    let mut reports = 0;
    loop {
        match stream.next().await {
            Some(Action::Output(Message::CloneProgress(_))) => reports += 1,
            Some(Action::Output(m @ Message::CloneDone(_))) => {
                let _ = ui.send(m).await;
                break;
            }
            Some(_) => {}
            None => panic!("ended without CloneDone"),
        }
    }
    assert!(reports > 1, "progress kept coming: {reports}");
    assert_eq!(ui.app.tabs.len(), 1, "opened");
}
