#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use common::{open, runner};
use gf_core::{CommandPhase, CoreEvent, EventBus, GfError};
use gf_git::{CancellationToken, GitCommand, RunOptions};
use gf_test_support::TestRepo;
use tokio::sync::mpsc;

#[tokio::test]
async fn generic_run_executes_any_subcommand() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    let r = open(&repo).await;

    let out = r.run(["rev-list", "--count", "HEAD"]).await.unwrap();
    assert_eq!(out.stdout_str().trim(), "1");

    let out = r.run(["count-objects", "-v"]).await.unwrap();
    assert!(out.stdout_str().contains("count:"));
}

#[tokio::test]
async fn failures_carry_exit_code_and_stderr() {
    let repo = TestRepo::new();
    let r = open(&repo).await;
    let err = r.run(["switch", "no-such-branch"]).await.unwrap_err();
    match err {
        GfError::GitCommand {
            argv,
            exit_code,
            stderr,
        } => {
            assert_eq!(argv, ["switch", "no-such-branch"]);
            assert_ne!(exit_code, Some(0));
            assert!(stderr.contains("no-such-branch"), "{stderr}");
        }
        other => panic!("unexpected error: {other}"),
    }

    let out = r
        .run_command(
            &r.command(["switch", "no-such-branch"]).allow_failure(),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(!out.success());
}

#[tokio::test]
async fn publishes_command_events() {
    let repo = TestRepo::new();
    let bus = EventBus::default();
    let mut events = bus.subscribe();
    let runner = runner(&repo).with_event_bus(bus);
    let r = gf_git::Repository::open(repo.path(), runner).await.unwrap();

    r.run(["status", "--short", "--branch"]).await.unwrap();

    let mut phases = Vec::new();
    while let Ok(CoreEvent::Command(e)) = events.try_recv() {
        phases.push(e);
    }
    // open() runs two rev-parse calls before our command.
    let ours: Vec<_> = phases.iter().filter(|e| e.repo == Some(r.id())).collect();
    let id = ours[0].id;
    assert!(ours.iter().all(|e| e.id == id));
    assert!(
        matches!(&ours[0].phase, CommandPhase::Started { argv, read_only: false } if argv[1] == "status")
    );
    assert!(
        ours.iter()
            .any(|e| matches!(&e.phase, CommandPhase::Output { line, is_stderr: false } if line.contains("main")))
    );
    assert!(matches!(
        ours.last().unwrap().phase,
        CommandPhase::Finished {
            exit_code: Some(0),
            ..
        }
    ));
}

#[tokio::test]
async fn cancellation_kills_the_process() {
    let repo = TestRepo::new();
    let runner = runner(&repo);
    let cmd = GitCommand::new(["-c", "alias.slow=!sleep 10", "slow"]).current_dir(repo.path());

    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        trigger.cancel();
    });

    let started = Instant::now();
    let result = runner
        .run_with(
            &cmd,
            RunOptions {
                cancel: Some(cancel),
                progress: None,
            },
        )
        .await;
    assert!(matches!(result, Err(GfError::Cancelled)), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn reports_clone_progress() {
    let origin = TestRepo::new();
    for i in 0..20 {
        origin.write(format!("f{i}.txt"), &format!("{i}\n"));
        origin.commit_all(&format!("c{i}"));
    }
    let target = tempfile::tempdir().unwrap();
    let url = format!("file://{}", origin.path().display());

    let (tx, mut rx) = mpsc::unbounded_channel();
    let cmd = GitCommand::new(["clone", "--progress", &url, "."]).current_dir(target.path());
    runner(&origin)
        .run_with(
            &cmd,
            RunOptions {
                cancel: None,
                progress: Some(tx),
            },
        )
        .await
        .unwrap();

    let mut updates = Vec::new();
    while let Ok(p) = rx.try_recv() {
        updates.push(p);
    }
    assert!(!updates.is_empty(), "no progress parsed");
    assert!(updates.iter().any(|p| p.percent == 100));
    assert!(target.path().join("f19.txt").exists());
}
