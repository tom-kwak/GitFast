//! gf entry point. The iced window arrives in M2; for now this wires up
//! storage paths and logging so every later milestone starts from them.

mod logging;

use anyhow::Context;
use gf_config::AppPaths;

fn main() -> anyhow::Result<()> {
    let paths = AppPaths::resolve().context("resolving app directories")?;
    paths.ensure_exist().context("creating app directories")?;
    let _log_guard = logging::init(&paths.log_dir());

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %paths.config_dir().display(),
        logs = %paths.log_dir().display(),
        "gf starting"
    );
    Ok(())
}
