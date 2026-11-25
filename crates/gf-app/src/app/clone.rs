//! Cloning a repository from a URL typed in the open window (⌘O): ask
//! where it goes, clone it, open it in a tab. The status line shows the
//! progress with `[ cancel Esc ]`; the app stays usable meanwhile. A
//! server that wants a login asks for one and the clone runs again (see
//! `settings.rs`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gf_config::DEFAULT_CLONE_ROOT;
use gf_git::{CancellationToken, GitCommand, Progress, RunOptions};
use iced::Task;

use super::dialogs::Purpose;
use super::settings::Retry;
use super::{App, Message};
use crate::features::command_log::NoteKind;
use crate::features::dialog::Dialog;
use crate::git_actions;
use crate::widgets::format;

/// A clone in progress.
#[derive(Debug)]
pub struct Cloning {
    /// `clone app`: what the status line and messages say.
    pub label: String,
    pub progress: Option<Progress>,
    pub token: CancellationToken,
    pub started: Instant,
}

impl Cloning {
    pub fn new(label: String, token: CancellationToken) -> Self {
        Self {
            label,
            progress: None,
            token,
            started: Instant::now(),
        }
    }

    /// `cloning app · Receiving objects 45% (450/1000)`; before git
    /// reports a percentage (the server packs the objects first, which can
    /// take a while) how long it has been: `cloning app… 3s`.
    pub fn status(&self) -> String {
        let name = self.label.strip_prefix("clone ").unwrap_or(&self.label);
        match &self.progress {
            Some(p) => format!(
                "cloning {name} · {} {}% ({}/{})",
                p.phase, p.percent, p.current, p.total
            ),
            None => match self.started.elapsed() {
                e if e < Duration::from_secs(1) => format!("cloning {name}…"),
                e => format!("cloning {name}… {}s", e.as_secs()),
            },
        }
    }
}

/// How a clone ended.
#[derive(Debug, Clone)]
pub struct CloneDone {
    pub url: String,
    pub folder: String,
    pub dest: PathBuf,
    pub result: Result<(), String>,
}

impl App {
    /// Prefills the clone destination from the configured clone folder and the URL.
    pub(super) fn clone_dialog(&mut self, url: String) -> Task<Message> {
        let folder = suggested_clone_folder(self.settings.repos.clone_root.as_deref(), &url);
        self.open_dialog(
            Dialog::confirm(
                "Clone repository",
                vec!["Copies the repository from the server, then opens it in a tab.".into()],
                "Clone",
                Purpose::Clone,
            )
            .field("URL", url.trim(), "https://github.com/team/app.git")
            .field("into folder", folder, format!("{DEFAULT_CLONE_ROOT}/app")),
        )
    }

    /// Updates the suggestion while the user leaves the destination at its default.
    pub(super) fn sync_clone_destination(&mut self, previous_url: Option<&str>) {
        let Some(dialog) = self
            .dialog
            .as_mut()
            .filter(|d| matches!(d.purpose, Purpose::Clone))
        else {
            return;
        };
        let [url, folder] = dialog.fields.as_mut_slice() else {
            return;
        };
        let Some(previous_url) = previous_url.filter(|previous| *previous != url.value) else {
            return;
        };
        let root = self.settings.repos.clone_root.as_deref();
        if folder.value.is_empty() || folder.value == suggested_clone_folder(root, previous_url) {
            folder.value = suggested_clone_folder(root, &url.value);
        }
    }

    /// Clones `url` into `folder` in the background, with progress.
    pub(super) fn run_clone(&mut self, url: &str, folder: &str) -> Task<Message> {
        if let Some(running) = &self.cloning {
            let busy = format!("{} is still running — Esc cancels it", running.label);
            self.flash(NoteKind::Error, busy);
            return Task::none();
        }
        let dest = format::expand_tilde(folder);
        let name = git_actions::humanish_name(url)
            .or_else(|| dest.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "repository".into());
        let token = CancellationToken::new();
        self.cloning = Some(Cloning::new(format!("clone {name}"), token.clone()));
        // git creates the missing folders on the way; run it from the
        // nearest one that exists.
        let cwd = dest
            .ancestors()
            .skip(1)
            .find(|p| p.is_dir())
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        let cmd = GitCommand::new(["clone", "--progress"])
            .arg(url)
            .arg(&dest)
            .current_dir(cwd);
        let runner = self.runner.clone();
        let (url, folder) = (url.to_owned(), folder.to_owned());
        let stream = iced::stream::channel(
            64,
            async move |mut out: iced::futures::channel::mpsc::Sender<Message>| {
                use iced::futures::SinkExt;
                let existed = dest.exists();
                let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel();
                let opts = RunOptions {
                    cancel: Some(token),
                    progress: Some(ptx),
                    ..RunOptions::default()
                };
                let run = runner.run_with(&cmd, opts);
                tokio::pin!(run);
                let result = loop {
                    tokio::select! {
                        Some(p) = prx.recv() => {
                            let _ = out.send(Message::CloneProgress(p)).await;
                        }
                        r = &mut run => break r,
                    }
                };
                if result.is_err() {
                    // A killed git cannot clean up after itself.
                    discard_partial_clone(&dest, existed);
                }
                let done = CloneDone {
                    url,
                    folder,
                    dest,
                    result: result.map(|_| ()).map_err(|e| e.to_string()),
                };
                let _ = out.send(Message::CloneDone(Box::new(done))).await;
            },
        );
        Task::run(stream, |m| m)
    }

    pub(super) fn clone_progress(&mut self, progress: Progress) -> Task<Message> {
        if let Some(c) = self.cloning.as_mut() {
            c.progress = Some(progress);
        }
        Task::none()
    }

    /// Esc / `[ cancel ]` in the status line. Whether a clone was running.
    pub(super) fn cancel_clone(&mut self) -> bool {
        let Some(c) = &self.cloning else {
            return false;
        };
        c.token.cancel();
        self.flash(NoteKind::Info, format!("cancelling {}…", c.label));
        true
    }

    pub(super) fn clone_done(&mut self, done: CloneDone) -> Task<Message> {
        let label = self
            .cloning
            .take()
            .map_or_else(|| "clone".to_owned(), |c| c.label);
        match done.result {
            Ok(()) => {
                self.flash(NoteKind::Info, format!("✓ {label}"));
                self.update(Message::Open(done.dest))
            }
            Err(e) if e.contains("operation cancelled") => {
                self.flash(NoteKind::Info, format!("{label} cancelled"));
                Task::none()
            }
            Err(e) => {
                self.log
                    .note(NoteKind::Error, format!("{label} failed"), vec![e.clone()]);
                self.flash(NoteKind::Error, format!("✗ {label} — see log"));
                if git_actions::needs_login(&e) && git_actions::auth_server(&e).is_some() {
                    let retry = Retry {
                        label: label.clone(),
                        purpose: Purpose::Clone,
                        values: vec![done.url, done.folder],
                    };
                    self.ask_login_for(&label, &e, Some(retry))
                } else {
                    self.show_failure(&label, &e)
                }
            }
        }
    }
}

/// Removes what a failed or cancelled clone left in `dest`: the folder,
/// or only its contents when it was there (empty) before.
fn discard_partial_clone(dest: &Path, existed: bool) {
    if !existed {
        let _ = std::fs::remove_dir_all(dest);
        return;
    }
    for entry in std::fs::read_dir(dest).into_iter().flatten().flatten() {
        let path = entry.path();
        let _ = if path.is_dir() && !path.is_symlink() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

fn suggested_clone_folder(root: Option<&Path>, url: &str) -> String {
    let default_root;
    let root = match root {
        Some(root) => root,
        None => {
            default_root = format::expand_tilde(DEFAULT_CLONE_ROOT);
            &default_root
        }
    };
    let url = url.trim();
    let name = git_actions::is_clone_url(url)
        .then(|| git_actions::humanish_name(url))
        .flatten()
        .filter(|name| {
            Path::new(name)
                .file_name()
                .is_some_and(|part| part == name.as_str())
        })
        .unwrap_or_else(|| "repository".into());
    format::tilde(&root.join(name))
}

/// What the clone dialog's fields (URL, folder) lack, if anything.
pub(super) fn check_clone(values: &[String]) -> Option<String> {
    let value = |i: usize| values.get(i).map(String::as_str).unwrap_or_default();
    let (url, folder) = (value(0), value(1));
    if url.is_empty() {
        return Some("enter the repository's URL".into());
    }
    if !git_actions::is_clone_url(url) {
        return Some(format!(
            "`{url}` is not a URL git clones from (https://… or git@host:path)"
        ));
    }
    if folder.is_empty() {
        return Some("choose a folder".into());
    }
    let dest = format::expand_tilde(folder);
    let taken = dest.is_file()
        || std::fs::read_dir(&dest).is_ok_and(|mut entries| entries.next().is_some());
    taken.then(|| format!("{folder} already exists and is not empty"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_clone_leaves_nothing_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let made = dir.path().join("made");
        std::fs::create_dir_all(made.join(".git/objects")).expect("mkdir");
        discard_partial_clone(&made, false);
        assert!(!made.exists(), "a folder the clone made goes");

        let kept = dir.path().join("kept");
        std::fs::create_dir_all(kept.join(".git")).expect("mkdir");
        std::fs::write(kept.join("a.txt"), "x").expect("write");
        discard_partial_clone(&kept, true);
        assert!(kept.is_dir(), "a folder that was there stays");
        assert_eq!(
            std::fs::read_dir(&kept).expect("read").count(),
            0,
            "emptied"
        );
    }

    #[test]
    fn the_status_line_says_how_far_the_clone_is() {
        let mut c = Cloning::new("clone app".into(), CancellationToken::new());
        assert_eq!(c.status(), "cloning app…");
        c.started -= Duration::from_secs(3);
        assert_eq!(c.status(), "cloning app… 3s", "the server is still packing");
        c.progress = Progress::parse("Receiving objects:  45% (450/1000), 1.20 MiB");
        assert_eq!(c.status(), "cloning app · Receiving objects 45% (450/1000)");
    }

    fn values(url: &str, folder: &str) -> Vec<String> {
        vec![url.to_owned(), folder.to_owned()]
    }

    #[test]
    fn a_clone_needs_a_url_and_a_free_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let free = dir.path().join("app").display().to_string();
        assert_eq!(
            check_clone(&values("https://github.com/a/app.git", &free)),
            None
        );
        assert_eq!(
            check_clone(&values("git@github.com:a/app.git", &free)),
            None
        );
        assert!(check_clone(&values("", &free)).is_some());
        assert!(check_clone(&values("~/work/app", &free)).is_some());
        assert!(check_clone(&values("https://github.com/a/app.git", "")).is_some());

        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).expect("mkdir");
        let empty = empty.display().to_string();
        assert_eq!(
            check_clone(&values("https://github.com/a/app.git", &empty)),
            None,
            "git clones into an empty folder"
        );
        std::fs::write(dir.path().join("empty/x"), "x").expect("write");
        assert!(
            check_clone(&values("https://github.com/a/app.git", &empty))
                .is_some_and(|e| e.contains("not empty"))
        );
    }
}
