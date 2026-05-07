//! wispertype library entry. The binary in `main.rs` calls into `run()`.
//!
//! Tauri 2.x's mobile/desktop split expects the app body to live in a lib so
//! mobile targets can re-use the same setup. We follow the same convention here.

mod app;
mod error;

use app::AppState;
use tauri::Manager;
use tracing::info;

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
        .setup(|app| {
            let settings = app
                .get_webview_window("settings")
                .expect("settings window must exist");
            settings.show().expect("show settings window");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
