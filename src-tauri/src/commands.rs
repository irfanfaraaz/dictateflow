//! Tauri command handlers reachable from the frontend via `invoke()`.
//!
//! Every command returns `AppResult<T>`; errors serialize to a string via
//! `AppError::serialize`. Internal modules use their own `thiserror` enums
//! and convert via `?`, picking up the "context for free" from anyhow.
//!
//! # Threading
//!
//! All commands are `async`; Tauri schedules them on the Tokio runtime.
//! Filesystem + plugin-store IO is fast enough that we do not push it to a
//! separate `spawn_blocking`. The `SettingsStore` itself never holds a
//! lock across an `.await`.

use anyhow::Context;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::error::AppResult;
use crate::permissions::{self, PermissionKind, PermissionState};
use crate::settings::{Settings, SettingsStore};

/// Event emitted Rust → frontend whenever any setting changes. Carries the
/// *whole* settings snapshot so consumers don't need to merge patches
/// themselves.
const EV_SETTINGS_UPDATED: &str = "settings:updated";

/// Snapshot of all three TCC permissions wispertype cares about.
///
/// Mirrors the zod schema in `src/schemas/settings.ts`. Reported in one
/// command so the frontend can poll cheaply on a 2 s timer without three
/// round-trips.
#[derive(Debug, Serialize, Clone)]
pub struct PermissionsSnapshot {
    pub microphone: PermissionState,
    pub accessibility: PermissionState,
    pub input_monitoring: PermissionState,
}

#[tauri::command]
pub async fn get_settings(store: State<'_, SettingsStore>) -> AppResult<Settings> {
    Ok(store.get())
}

#[tauri::command]
pub async fn update_settings<R: Runtime>(
    app: AppHandle<R>,
    store: State<'_, SettingsStore>,
    patch: Value,
) -> AppResult<Settings> {
    let next = store.update_patch(&app, patch).context("update settings")?;

    // Emit AFTER the store mutation so listeners on other windows can rely
    // on the snapshot reflecting disk state. Failures here are non-fatal —
    // the change is durable; we just lost the cross-window heads-up.
    if let Err(e) = app.emit(EV_SETTINGS_UPDATED, next.clone()) {
        tracing::warn!(?e, "emit settings:updated failed");
    }

    Ok(next)
}

#[tauri::command]
pub async fn get_permissions() -> AppResult<PermissionsSnapshot> {
    Ok(PermissionsSnapshot {
        microphone: permissions::check_microphone(),
        accessibility: permissions::check_accessibility(),
        input_monitoring: permissions::check_input_monitoring(),
    })
}

#[tauri::command]
pub async fn open_system_settings(kind: PermissionKind) -> AppResult<()> {
    permissions::open_system_settings(kind);
    Ok(())
}

/// Open an arbitrary path in the user's default handler. Used by the About
/// tab to surface the log file (Phase 9 wires the writer; the path link
/// works today and shows an empty file).
#[tauri::command]
pub async fn open_path(path: String) -> AppResult<()> {
    // `std::process::Command::new("open")` is the canonical mac shell-out
    // for "do whatever the user has registered for this URI/path." Tauri
    // ships its own `shell` plugin for the same job, but we don't want to
    // pull a whole capability surface for one call.
    let status = std::process::Command::new("open")
        .arg(&path)
        .status()
        .with_context(|| format!("spawn `open` for {path}"))?;
    if !status.success() {
        return Err(anyhow::anyhow!("`open {path}` exited with {status}").into());
    }
    Ok(())
}

/// Save the ElevenLabs API key to the macOS Keychain and set
/// `settings.scribe.has_api_key = true`.
///
/// The raw key is NEVER logged — tracing calls in this function must not
/// include `key` in any field. The frontend only ever sees `has_api_key: bool`.
#[tauri::command]
pub async fn set_api_key<R: Runtime>(
    app: AppHandle<R>,
    store: State<'_, SettingsStore>,
    key: String,
) -> AppResult<()> {
    if key.trim().is_empty() {
        return Err(anyhow::anyhow!("api key cannot be empty").into());
    }
    crate::keychain::set(crate::keychain::ELEVENLABS_KEY, key.trim())
        .map_err(|e| anyhow::anyhow!("keychain set: {e}"))?;

    // Persist the flag (not the secret) in settings.json, then notify the UI.
    let next = store
        .update_patch(&app, json!({"scribe": {"has_api_key": true}}))
        .context("update settings after set_api_key")?;

    // Clear any cached Scribe engine so the next dictation picks up the new key.
    if let Some(state) = app.try_state::<crate::app::AppState>() {
        state.clear_engine();
    }

    if let Err(e) = app.emit(EV_SETTINGS_UPDATED, next) {
        tracing::warn!(?e, "emit settings:updated after set_api_key failed");
    }
    Ok(())
}

/// Remove the ElevenLabs API key from Keychain and clear `has_api_key`.
#[tauri::command]
pub async fn clear_api_key<R: Runtime>(
    app: AppHandle<R>,
    store: State<'_, SettingsStore>,
) -> AppResult<()> {
    crate::keychain::delete(crate::keychain::ELEVENLABS_KEY)
        .map_err(|e| anyhow::anyhow!("keychain delete: {e}"))?;

    let next = store
        .update_patch(&app, json!({"scribe": {"has_api_key": false}}))
        .context("update settings after clear_api_key")?;

    // Evict cached Scribe engine.
    if let Some(state) = app.try_state::<crate::app::AppState>() {
        state.clear_engine();
    }

    if let Err(e) = app.emit(EV_SETTINGS_UPDATED, next) {
        tracing::warn!(?e, "emit settings:updated after clear_api_key failed");
    }
    Ok(())
}

/// Run `health_check()` on the engine the current settings would build.
/// Returns `true` if healthy, propagates the engine error as `AppError`
/// if not.
#[tauri::command]
pub async fn test_engine<R: Runtime>(
    app: AppHandle<R>,
    store: State<'_, SettingsStore>,
) -> AppResult<bool> {
    let settings = store.get();
    let engine = crate::engine::factory::build(&app, &settings)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    engine
        .health_check()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(true)
}

/// Resolve a known app file path the frontend can pass back to `open_path`.
/// We expose it as a command rather than computing in the renderer because
/// `path::PathResolver` is Rust-only.
#[tauri::command]
pub async fn resolve_log_path<R: Runtime>(app: AppHandle<R>) -> AppResult<String> {
    // `~/Library/Logs/wispertype/wispertype.log`. The directory is created
    // on first call so `open` doesn't fail with ENOENT before Phase 9 wires
    // the writer.
    let home = app.path().home_dir().context("resolve home dir")?;
    let log_dir = home.join("Library/Logs/wispertype");
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        tracing::warn!(?e, "create log dir failed (non-fatal)");
    }
    let log_file = log_dir.join("wispertype.log");
    // Touch the file so `open` has something to launch.
    if !log_file.exists() {
        if let Err(e) = std::fs::write(&log_file, "") {
            tracing::warn!(?e, "touch log file failed (non-fatal)");
        }
    }
    Ok(log_file.to_string_lossy().into_owned())
}
