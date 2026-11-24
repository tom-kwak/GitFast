//! Where a sweep's time goes. With `GF_SWEEP_PROFILE=1` every sweep
//! prints, when it ends, the time spent in each phase (building the
//! fixture, opening it, rendering frames, comparing state, ...).
//!
//! Phases nest (`reach` holds `reach › build`, which holds `open`), so
//! their shares add up to more than the whole.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Time spent in one phase and how often it ran.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(super) struct Phase {
    pub total: Duration,
    pub count: u32,
}

std::thread_local! {
    /// The running test's phases (tests run on one thread each).
    static PHASES: RefCell<BTreeMap<&'static str, Phase>> = RefCell::default();
}

/// Whether `GF_SWEEP_PROFILE` asks for the report.
pub(super) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("GF_SWEEP_PROFILE").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// Counts `elapsed` toward `name` (for a phase not run as one closure).
pub(super) fn record(name: &'static str, elapsed: Duration) {
    if enabled() {
        add(name, elapsed);
    }
}

fn add(name: &'static str, elapsed: Duration) {
    PHASES.with_borrow_mut(|phases| {
        let phase = phases.entry(name).or_default();
        phase.total += elapsed;
        phase.count += 1;
    });
}

/// Runs `f`, counting its time toward `name`.
pub(super) fn time<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let start = Instant::now();
    let out = f();
    add(name, start.elapsed());
    out
}

/// Awaits `f`, counting its time toward `name`.
pub(super) async fn time_async<T>(name: &'static str, f: impl Future<Output = T>) -> T {
    if !enabled() {
        return f.await;
    }
    let start = Instant::now();
    let out = f.await;
    add(name, start.elapsed());
    out
}

/// The phases counted on this thread so far, cleared.
pub(super) fn take() -> BTreeMap<&'static str, Phase> {
    PHASES.with_borrow_mut(std::mem::take)
}

/// The report: one line per phase, the slowest first.
pub(super) fn report(
    sweep: &str,
    total: Duration,
    phases: &BTreeMap<&'static str, Phase>,
) -> String {
    let mut rows: Vec<_> = phases.iter().collect();
    rows.sort_by(|a, b| b.1.total.cmp(&a.1.total).then(a.0.cmp(b.0)));
    let mut out = format!(
        "sweep profile: {sweep} — {:.2}s (phases nest; shares overlap)\n",
        total.as_secs_f64()
    );
    for (name, phase) in rows {
        let share = if total.is_zero() {
            0.0
        } else {
            phase.total.as_secs_f64() / total.as_secs_f64() * 100.0
        };
        let avg_ms = phase.total.as_secs_f64() * 1000.0 / f64::from(phase.count.max(1));
        out.push_str(&format!(
            "  {name:<22} {:>8.2}s {share:>5.1}%  ×{:<6} {avg_ms:>8.2}ms avg\n",
            phase.total.as_secs_f64(),
            phase.count,
        ));
    }
    out
}

#[test]
fn report_lists_the_slowest_phase_first_with_its_share_and_average() {
    let phases = BTreeMap::from([
        (
            "render",
            Phase {
                total: Duration::from_millis(300),
                count: 3,
            },
        ),
        (
            "reach",
            Phase {
                total: Duration::from_millis(600),
                count: 2,
            },
        ),
    ]);
    let text = report("history", Duration::from_secs(1), &phases);
    let lines: Vec<_> = text.lines().collect();
    assert!(lines[0].starts_with("sweep profile: history — 1.00s"));
    assert!(lines[1].trim_start().starts_with("reach"));
    assert!(lines[1].contains("0.60s"));
    assert!(lines[1].contains("60.0%"));
    assert!(lines[1].contains("×2"));
    assert!(lines[1].contains("300.00ms avg"));
    assert!(lines[2].trim_start().starts_with("render"));
    assert!(lines[2].contains("100.00ms avg"));
}

#[test]
fn time_counts_nothing_unless_enabled() {
    // The variable is not set for `cargo test`; with it set, the phase counts.
    let _ = take();
    assert_eq!(time("x", || 7), 7);
    assert_eq!(take().contains_key("x"), enabled());
}
