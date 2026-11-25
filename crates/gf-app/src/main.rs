//! gf entry point: storage paths, logging, settings, then the iced app.

// Release builds on Windows are GUI apps: no console window next to gf.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod badges;
mod feature;
mod features;
mod git_actions;
mod highlight;
mod hosting;
mod logging;
mod nav;
mod platform;
mod repo_tab;
mod theme;
mod undo;
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
    .default_font(platform::mono_font())
    .window(platform::window_settings())
    .run()
    .context("running the UI")?;
    Ok(())
}
