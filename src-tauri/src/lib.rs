//! wispertype library entry. The binary in `main.rs` calls into `run()`.
//!
//! Tauri 2.x's mobile/desktop split expects the app body to live in a lib so
//! mobile targets can re-use the same setup. We follow the same convention here.

mod app;
mod audio;
mod error;
mod events;
mod hotkey;
mod permissions;
mod window;

use app::{AppState, DictationController};
use tauri::Manager;
use tracing::{info, warn};

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
        .setup(|app| {
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
