//! Read-only timing of the git CLI against gix on a real repository:
//!
//! ```bash
//! GF_BENCH_REPO=/path/to/big/repo cargo test -p gf-git --release --test gix_bench -- --ignored --nocapture
//! ```
//!
//! Each read runs a few times per backend; the best time is printed, and
//! `MISMATCH` when the two answers differ.
//! `GIT_OPTIONAL_LOCKS=0` keeps `git status` from refreshing the index.
#![cfg(feature = "gix")]
#![allow(clippy::unwrap_used, clippy::print_stdout)]

use std::future::Future;
use std::time::{Duration, Instant};

use gf_git::backend::{DiffOps, LogOps, RefOps, StashOps, StatusOps};
use gf_git::{GitRunner, LogOrder, LogQuery, Repository};

const RUNS: usize = 5;

/// Best of [`RUNS`] and the last answer; the error when the backend
/// cannot answer.
async fn best<T, Fut: Future<Output = gf_core::Result<T>>>(
    f: impl Fn() -> Fut,
) -> Result<(Duration, T), String> {
    let mut best = Duration::MAX;
    let mut last = None;
    for _ in 0..RUNS {
        let t = Instant::now();
        last = Some(f().await.map_err(|e| e.to_string())?);
        best = best.min(t.elapsed());
    }
    Ok((best, last.expect("RUNS > 0")))
}

macro_rules! bench {
    ($repo:expr, $name:expr, $tr:ident :: $m:ident ( $($arg:expr),* )) => {{
        let (cli, gix) = $repo.backends();
        let gix = gix.unwrap();
        let (c, expected) = best(|| $tr::$m(cli, $($arg),*)).await.unwrap();
        match best(|| $tr::$m(gix, $($arg),*)).await {
            Ok((g, got)) => println!(
                "{:<24} cli {:>9.2?}   gix {:>9.2?}   x{:.1}{}",
                $name,
                c,
                g,
                c.as_secs_f64() / g.as_secs_f64(),
                if got == expected { "" } else { "   MISMATCH" }
            ),
            Err(e) => println!("{:<24} cli {:>9.2?}   gix -> CLI ({e})", $name, c),
        }
    }};
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs GF_BENCH_REPO"]
async fn compare_backends() {
    let Ok(path) = std::env::var("GF_BENCH_REPO") else {
        println!("set GF_BENCH_REPO to a repository path");
        return;
    };
    let runner = GitRunner::new().with_env([("GIT_OPTIONAL_LOCKS", "0")]);
    let repo = Repository::open(&path, runner).await.unwrap();
    let history = LogQuery {
        max_count: Some(1000),
        order: LogOrder::Topo,
        ..LogQuery::default()
    };
    let all = LogQuery {
        all_refs: true,
        ..history.clone()
    };
    println!("repository: {path}");
    bench!(repo, "status", StatusOps::status());
    bench!(repo, "numstat (unstaged)", DiffOps::numstat(false));
    bench!(repo, "numstat (staged)", DiffOps::numstat(true));
    bench!(repo, "log topo 1000", LogOps::log(&history));
    bench!(repo, "log topo 1000 --all", LogOps::log(&all));
    bench!(repo, "branches", RefOps::branches());
    bench!(repo, "tags", RefOps::tags());
    bench!(repo, "unpushed_count", RefOps::unpushed_count());
    bench!(repo, "stashes", StashOps::stashes());
}
