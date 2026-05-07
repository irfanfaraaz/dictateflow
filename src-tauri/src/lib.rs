//! wispertype library entry. The binary in `main.rs` calls into `run()`.
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
use tauri::Manager;
use tracing::{error, info, warn};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("wispertype=info")),
        )
        .init();

    info!("wispertype starting");

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::new())
        .manage(DictationController::new())
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::update_settings,
            commands::get_permissions,
            commands::open_system_settings,
            commands::open_path,
            commands::resolve_log_path,
            commands::set_api_key,
            commands::clear_api_key,
            commands::test_engine,
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

            // Register the F5 hold-to-talk hotkey. Failures here are
            // typically a missing Input Monitoring grant; we log and
            // continue so the settings window remains usable.
            if let Err(e) = hotkey::listener::register(app.handle()) {
                warn!(
                    ?e,
                    "failed to register F5 hotkey (input monitoring permission?)"
                );
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
