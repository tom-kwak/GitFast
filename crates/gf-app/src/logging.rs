//! Logging setup: human-readable output on stderr plus a daily-rotated file in
//! the app's log directory. Filter with `RUST_LOG` (default: `info`, and
//! `debug` for gf crates in debug builds).

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const DEFAULT_FILTER: &str = if cfg!(debug_assertions) {
    "info,gf=debug,gf_core=debug,gf_git=debug,gf_app=debug"
} else {
    "info"
};

/// Installs the global subscriber. Keep the returned guard alive for the whole
/// program; dropping it flushes buffered file output.
pub fn init(log_dir: &Path) -> WorkerGuard {
    let file_appender = tracing_appender::rolling::daily(log_dir, "gf.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .init();

    guard
}
