//! gf entry point: storage paths, logging, settings, then the iced app.

mod app;
mod feature;
mod features;
mod git_actions;
mod highlight;
mod logging;
mod repo_tab;
mod theme;
mod widgets;

use anyhow::Context;
use gf_config::{AppPaths, Settings};

fn main() -> anyhow::Result<()> {
    // GF_HOME keeps all settings/logs under one folder (portable use, testing).
    let paths = match std::env::var_os("GF_HOME") {
        Some(home) => AppPaths::under(std::path::Path::new(&home)),
        None => AppPaths::resolve().context("resolving app directories")?,
    };
    paths.ensure_exist().context("creating app directories")?;
    let _log_guard = logging::init(&paths.log_dir());

    let settings = Settings::load(&paths.settings_file()).unwrap_or_else(|e| {
        // A broken settings file must never keep the app from starting.
        tracing::warn!(error = %e, "invalid settings file; using defaults");
        Settings::default()
    });
    // Grammars load in the background; user grammars live in
    // <config>/syntaxes (VS Code *.tmLanguage.json or *.sublime-syntax).
    let syntaxes = paths.config_dir().join("syntaxes");
    let _ = std::fs::create_dir_all(&syntaxes);
    std::thread::spawn(move || {
        for warning in highlight::init(Some(&syntaxes)) {
            tracing::warn!(%warning, "grammar not loaded");
        }
    });

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %paths.config_dir().display(),
        "gf starting"
    );

    iced::application(
        move || app::App::boot(paths.clone(), settings.clone()),
        app::App::update,
        app::App::view,
    )
    .title(app::App::title)
    .theme(app::App::theme)
    .subscription(app::App::subscription)
    .default_font(iced::Font::MONOSPACE)
    .window_size((1280.0, 820.0))
    .centered()
    .run()
    .context("running the UI")?;
    Ok(())
}
