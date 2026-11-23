//! Memory benchmark: drives the real `App` headless with several
//! repositories open in tabs and prints how much heap each step keeps.
//!
//! Compiled into gf-app's test binary only with the `mem-bench` feature
//! (see `crates/gf-app/src/app/mod.rs`). How to run and read it:
//! `tools/mem-bench/README.md`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use iced::futures::StreamExt;
use iced_test::runtime::{self, Action};

use gf_core::model::Oid;

use super::*;
use crate::repo_tab;

/// Counts every allocation of the test binary (all threads).
#[global_allocator]
static HEAP: cap::Cap<std::alloc::System> = cap::Cap::new(std::alloc::System, usize::MAX);

const MB: f64 = 1024.0 * 1024.0;

/// Bytes allocated and not yet freed, in MB.
fn live() -> f64 {
    HEAP.allocated() as f64 / MB
}

/// Most bytes alive at once since the process started, in MB.
fn high_water() -> f64 {
    HEAP.max_allocated() as f64 / MB
}

/// Resident memory as the OS reports it, in MB (0 when unknown).
fn rss() -> f64 {
    memory_stats::memory_stats().map_or(0.0, |m| m.physical_mem as f64 / MB)
}

/// Runs `$body`, then prints the heap it kept, how long it took, and the
/// totals after it.
macro_rules! step {
    ($label:expr, $body:expr) => {{
        let (before, start) = (live(), Instant::now());
        let r = $body;
        println!(
            "{:<44} Δheap {:>+8.1} MB {:>8.0} ms | heap {:>7.1}  high-water {:>7.1}  rss {:>7.1}",
            $label,
            live() - before,
            start.elapsed().as_secs_f64() * 1000.0,
            live(),
            high_water(),
            rss()
        );
        r
    }};
}

/// Runs a message and every task it starts, as the iced runtime would.
async fn drive(app: &mut App, message: Message) {
    let mut tasks = VecDeque::from([app.update(message)]);
    while let Some(task) = tasks.pop_front() {
        let Some(mut stream) = runtime::task::into_stream(task) else {
            continue;
        };
        loop {
            match tokio::time::timeout(Duration::from_secs(300), stream.next()).await {
                Ok(Some(Action::Output(m))) => tasks.push_back(app.update(m)),
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    println!("  !! a task ran over 300 s; skipped");
                    break;
                }
            }
        }
    }
}

fn history(app: &App) -> Option<&repo_tab::History> {
    let tab = app.tabs.get(app.active)?;
    Some(&tab.snapshot.as_ref()?.history)
}

fn history_len(app: &App) -> usize {
    history(app).map_or(0, |h| h.commits.len())
}

/// Time of `App::view()` (what iced rebuilds after every message), ms:
/// the first call after new data (it fills the view's memos) and the
/// average of the calls after it (every other frame).
fn view_ms(app: &App) -> (f64, f64) {
    let start = Instant::now();
    drop(app.view());
    let first = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    for _ in 0..20 {
        drop(app.view());
    }
    (first, start.elapsed().as_secs_f64() * 1000.0 / 20.0)
}

/// [`view_ms`] as text.
fn view_text(app: &App) -> String {
    let (first, then) = view_ms(app);
    format!("view() first {first:.2} ms, then {then:.2} ms")
}

/// The commit with the most changed lines among the newest 2000 (merges
/// left out), as a realistic "large diff".
fn largest_commit(repo: &Path) -> Option<(Oid, usize)> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "log",
            "--no-merges",
            "--shortstat",
            "--format=@%H",
            "-n",
            "2000",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut best: Option<(String, usize)> = None;
    let mut id = String::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix('@') {
            id = h.to_owned();
        } else if line.contains("changed") {
            let words: Vec<&str> = line.split_whitespace().collect();
            let lines: usize = words
                .windows(2)
                .filter(|w| w[1].starts_with("insertion") || w[1].starts_with("deletion"))
                .filter_map(|w| w[0].parse::<usize>().ok())
                .sum();
            if best.as_ref().is_none_or(|(_, n)| lines > *n) {
                best = Some((id.clone(), lines));
            }
        }
    }
    let (id, lines) = best?;
    Some((id.parse().ok()?, lines))
}

/// A repository whose last commit rewrites a 10 MB file (300k lines): the
/// worst case for a diff kept in a tab.
fn big_diff_repo() -> gf_test_support::TestRepo {
    let repo = gf_test_support::TestRepo::new();
    let mut seed = 1u64;
    let file = |seed: &mut u64| {
        let mut s = String::with_capacity(11 << 20);
        for i in 0..300_000 {
            // xorshift: different content on every call.
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            s.push_str(&format!("  \"key{i}\": \"{:x}\",\n", *seed));
        }
        s
    };
    repo.write("data.json", &file(&mut seed));
    repo.commit_all("base");
    repo.write("data.json", &file(&mut seed));
    repo.commit_all("rewrite");
    repo
}

/// Where the history's memory goes.
fn print_breakdown(h: &repo_tab::History) {
    let mb = |b: usize| b as f64 / MB;
    let n = h.commits.len().max(1);
    let strings: usize = h
        .commits
        .iter()
        .map(|c| {
            c.summary.capacity()
                + c.body.capacity()
                + c.author.name.capacity()
                + c.author.email.capacity()
                + c.committer.name.capacity()
                + c.committer.email.capacity()
                + c.parents.capacity() * std::mem::size_of::<Oid>()
        })
        .sum();
    let edges: usize = h.graph.iter().map(|r| r.edges.capacity()).sum();
    let width: usize = h.graph.iter().map(|r| r.width).sum();
    let max_width = h.graph.iter().map(|r| r.width).max().unwrap_or(0);
    println!("  breakdown of {} commits:", h.commits.len());
    println!(
        "    Commit structs  {:>8.1} MB",
        mb(h.commits.capacity() * std::mem::size_of::<gf_core::model::Commit>())
    );
    println!("    commit strings  {:>8.1} MB", mb(strings));
    println!(
        "    graph rows      {:>8.1} MB",
        mb(h.graph.capacity() * std::mem::size_of::<gf_git::graph::GraphRow>())
    );
    println!(
        "    graph edges     {:>8.1} MB ({} edges)",
        mb(edges * std::mem::size_of::<gf_git::graph::Edge>()),
        edges
    );
    println!(
        "    lanes per row   avg {:.1}, max {max_width}",
        width as f64 / n as f64
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "memory benchmark: tools/mem-bench/README.md"]
async fn mem_bench() {
    // One path per line (see README): works the same on every OS.
    let repos: Vec<PathBuf> = std::env::var("GF_BENCH_REPOS")
        .expect("GF_BENCH_REPOS: repositories to open, one path per line")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect();
    let deep: usize = match std::env::var("GF_BENCH_DEEP").as_deref() {
        Ok("all") => usize::MAX,
        Ok(n) => n.parse().expect("GF_BENCH_DEEP: a number or `all`"),
        Err(_) => 20_000,
    };

    let home = tempfile::tempdir().expect("home");
    let (mut app, _boot) = App::boot(
        gf_config::AppPaths::under(home.path()),
        gf_config::Settings::default(),
    );
    println!("gf mem-bench on {}", std::env::consts::OS);
    // Loaded once for the whole app: do not count it against a tab.
    step!("syntax highlighter (global, once)", {
        let _ = crate::highlight::get();
    });

    let synthetic = big_diff_repo();
    let mut all = repos.clone();
    all.push(synthetic.path().to_path_buf());

    for (i, path) in all.iter().enumerate() {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into(),
        );
        let name = if i == repos.len() {
            "synthetic 10 MB rewrite".to_owned()
        } else {
            name
        };
        println!("\n--- tab {}: {name}", i + 1);
        step!("open tab (first history page)", {
            drive(&mut app, Message::Open(path.clone())).await
        });
        let Some(head) = history(&app)
            .and_then(|h| h.commits.first())
            .map(|c| c.id.clone())
        else {
            println!("  (could not open; skipped)");
            continue;
        };
        println!(
            "  history {} commits, {}",
            history_len(&app),
            view_text(&app)
        );
        step!("select HEAD commit (its diff)", {
            drive(
                &mut app,
                Message::History(history::Message::Select(head.clone())),
            )
            .await
        });
        let large = if i == repos.len() {
            None
        } else {
            largest_commit(path).filter(|(id, _)| *id != head)
        };
        if let Some((id, lines)) = large {
            step!(format!("select largest commit ({lines} lines)"), {
                drive(&mut app, Message::History(history::Message::Select(id))).await
            });
        }
        println!("  with that diff: {}", view_text(&app));
        if i == repos.len() {
            // Past the large-diff size the view waits for ⏎: draw it too.
            drive(&mut app, Message::CommitDiff(diff_view::Message::ShowLarge)).await;
            println!("  shown anyway (⏎): {}", view_text(&app));
        }
        // What dropping a background tab's diff would give back.
        let tab = &mut app.tabs[app.active];
        step!("  if the commit diff were dropped", {
            tab.commit_diff = diff_view::State::new(diff_view::Role::Commit);
        });
    }

    if let Some(first) = repos.first() {
        println!("\n--- history scroll on {}", first.display());
        drive(&mut app, Message::Tabs(tabs::Message::Select(0))).await;
        let mut target = 5_000usize;
        loop {
            let goal = target.min(deep);
            let reached = step!(format!("load more until {goal} commits"), {
                while history_len(&app) < goal {
                    let before = history_len(&app);
                    drive(&mut app, Message::History(history::Message::LoadMore)).await;
                    if history_len(&app) == before {
                        break;
                    }
                }
                history_len(&app)
            });
            println!("  history {reached} commits, {}", view_text(&app));
            if reached < goal || goal == deep {
                break;
            }
            target *= 2;
        }
        if let Some(h) = history(&app) {
            print_breakdown(h);
        }
    }

    println!("\n--- close every tab");
    while !app.tabs.is_empty() {
        let name = app.tabs[0].name();
        step!(format!("close {name}"), {
            drive(&mut app, Message::Tabs(tabs::Message::Close(0))).await
        });
    }
    println!(
        "\nleft after closing: heap {:.1} MB, rss {:.1} MB",
        live(),
        rss()
    );
}
