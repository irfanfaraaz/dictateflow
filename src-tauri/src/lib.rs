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

    let app = tauri::Builder::default()
        // MUST be the first plugin: a second launch exits before any other
        // plugin (store, global shortcut, tray) initializes, so we never get
        // a duplicate tray icon, a failed hotkey registration, or two
        // processes writing settings.json. The running instance surfaces its
        // settings window instead.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            info!("second launch detected — focusing settings window");
            tray::show_settings_window(app);
        }))
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

            // The stored `has_api_key` flag is only a hint; the Keychain is
            // the source of truth (the item may have been removed in
            // Keychain Access, or settings.json restored from a backup).
            reconcile_api_key_flag(app.handle());

            // Start quietly in the tray once the app is set up; only open
            // the settings window on first run or when dictation can't work
            // yet (no API key / no installed model for the active engine).
            let show_settings = startup_needs_settings(app.handle());
            if show_settings {
                tray::show_settings_window(app.handle());
            } else {
                info!("setup complete — starting in the menu bar");
            }

            // Drop `.partial` downloads that can never be resumed. Partials
            // for catalog models are kept so the next download resumes.
            if let Some(dir) = models::models_dir(app.handle()) {
                tauri::async_runtime::spawn(async move {
                    models::download::cleanup_orphan_partials(&dir).await;
                });
            }

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
        .on_window_event(|window, event| {
            // Closing the settings window hides it instead of destroying it,
            // so the tray's "Open Settings" (and a second launch) can always
            // bring it back.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "settings" {
                    api.prevent_close();
                    if let Err(e) = window.hide() {
                        warn!(?e, "hide settings window failed");
                    }
                }
            }
        })
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(e) => {
            error!(?e, "failed to start DictateFlow");
            std::process::exit(1);
        }
    };

    app.run(|_app, _event| {
        // Clicking the Dock icon while no window is visible reopens settings.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } = _event
        {
            tray::show_settings_window(_app);
        }
    });
}

/// Make `settings.scribe.has_api_key` match the Keychain.
fn reconcile_api_key_flag<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let has_key = match keychain::has(keychain::ELEVENLABS_KEY) {
        Ok(v) => v,
        Err(e) => {
            // Keychain unavailable (locked, denied): keep the stored flag
            // rather than guessing.
            warn!(?e, "keychain check failed — keeping stored has_api_key");
            return;
        }
    };
    let Some(store) = app.try_state::<settings::SettingsStore>() else {
        return;
    };
    if store.get().scribe.has_api_key == has_key {
        return;
    }
    info!(has_key, "reconciling has_api_key with keychain");
    if let Err(e) = store.update_with(app, |s| s.scribe.has_api_key = has_key) {
        warn!(?e, "persist reconciled has_api_key failed");
    }
}

/// Decide whether the settings window should open at launch. Also marks
/// `first_run_completed` once a usable engine is configured, so subsequent
/// launches start quietly in the tray.
fn startup_needs_settings<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let Some(store) = app.try_state::<settings::SettingsStore>() else {
        return true;
    };
    let s = store.get();
    let usable = match s.engine {
        settings::schema::EngineKind::Local => models::resolve_path(app, &s.local.model).is_ok(),
        settings::schema::EngineKind::Scribe => s.scribe.has_api_key,
    };

    if usable && !s.first_run_completed {
        // Show it this one time (first run), but remember setup is done.
        if let Err(e) = store.update_with(app, |s| s.first_run_completed = true) {
            warn!(?e, "persist first_run_completed failed");
        }
        return true;
    }

    !usable || !s.first_run_completed
}
