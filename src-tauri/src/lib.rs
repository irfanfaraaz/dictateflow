//! DictateFlow library entry. The binary in `main.rs` calls into `run()`.
//!
//! Tauri 2.x's mobile/desktop split expects the app body to live in a lib so
//! mobile targets can re-use the same setup. We follow the same convention here.

mod app;
mod audio;
mod commands;
mod engine;
mod error;
mod events;
mod hotkey;
mod inject;
pub mod keychain;
mod models;
mod permissions;
mod settings;
mod sounds;
mod tray;
mod window;

/// Test-only re-exports.
///
/// Integration tests under `src-tauri/tests/` cannot reach into the crate's
/// private modules, but exposing engine internals as `pub` from `lib.rs`
/// would invite frontend code to import them too. This `#[doc(hidden)]`
/// module is the compromise: it is technically `pub`, but explicitly named
/// `__test_only` so callers know it's not a stability surface.
#[doc(hidden)]
pub mod __test_only {
    pub use crate::engine::{local::LocalEngine, AudioChunk, Engine, EngineError, Partial};
}

use app::{AppState, DictationController};
use hotkey::listener::HotkeyManager;
use tauri::Manager;
use tracing::{error, info, warn};
use tracing_subscriber::prelude::*;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // --- File logger setup ---
    // Rolling daily log to ~/Library/Logs/dictateflow/dictateflow.log.YYYY-MM-DD
    // The non-blocking writer guard must outlive the program. Binding it to a
    // `let` inside `run()` is safe because `tauri::Builder::run()` blocks
    // until quit — the binding is live for the entire app lifetime.
    let log_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("Library/Logs/dictateflow");
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::daily(&log_dir, "dictateflow.log");
    let (non_blocking_file, _file_guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("dictateflow=info"));

    // Stdout layer (existing dev experience) + file layer.
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(env_filter.clone()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking_file)
                .with_ansi(false)
                .with_filter(env_filter),
        )
        .init();

    info!("dictateflow starting");

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::new())
        .manage(DictationController::new())
        .manage(HotkeyManager::new())
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::update_settings,
            commands::get_permissions,
            commands::open_system_settings,
            commands::open_path,
            commands::resolve_log_path,
            commands::open_log_dir,
            commands::set_api_key,
            commands::clear_api_key,
            commands::test_engine,
            commands::download_model,
            commands::cancel_model_download,
            commands::delete_model,
            commands::list_installed_models,
            commands::list_input_devices,
        ])
        .setup(|app| {
            // Settings store load happens in setup so the AppHandle is
            // available. Failure here is fatal — without persistence the
            // settings UI would silently drop user changes, which is worse
            // than refusing to launch.
            match settings::SettingsStore::load(app.handle()) {
                Ok(store) => {
                    app.manage(store);
                }
                Err(e) => {
                    error!(?e, "failed to load settings store");
                    return Err(Box::new(e));
                }
            }

            let settings = app
                .get_webview_window("settings")
                .expect("settings window must exist");
            settings.show().expect("show settings window");

            // Register the configured hotkey (falls back to F5 on parse/OS
            // failure). Requires Input Monitoring permission on macOS; we log
            // and continue so the settings window stays usable even if the
            // permission hasn't been granted yet.
            if let Err(e) = hotkey::listener::register_from_settings(app.handle()) {
                warn!(
                    ?e,
                    "failed to register hotkey (input monitoring permission?)"
                );
            }

            // Build the menu bar tray icon. Removes the need for the static
            // `trayIcon` conf entry (which would create a second icon).
            if let Err(e) = tray::build(app.handle()) {
                warn!(?e, "tray icon build failed (non-fatal)");
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
