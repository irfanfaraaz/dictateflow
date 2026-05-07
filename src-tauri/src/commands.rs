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

use std::sync::Arc;

use anyhow::Context;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio_util::sync::CancellationToken;

use crate::error::AppResult;
use crate::hotkey::listener::HotkeyManager;
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
    // If the patch touches `hotkey.binding`, validate + rebind BEFORE
    // persisting so a bad chord is rejected without dirtying the store.
    if let Some(obj) = patch.as_object() {
        if let Some(hotkey_val) = obj.get("hotkey") {
            if let Some(binding) = hotkey_val.get("binding").and_then(|v| v.as_str()) {
                crate::hotkey::validate_binding(binding).map_err(|e| anyhow::anyhow!("{e}"))?;

                // Apply rebind immediately. On failure the old binding stays active;
                // we still persist the validated value (it parsed OK, just the OS
                // registration failed — likely a permission issue).
                if let Some(mgr) = app.try_state::<Arc<HotkeyManager>>() {
                    if let Err(e) = mgr.rebind(&app, binding) {
                        tracing::warn!(?e, "rebind failed — old hotkey preserved");
                        // Reject the update so the UI stays consistent with the active binding.
                        return Err(anyhow::anyhow!("rebind failed: {e}").into());
                    }
                }
            }
        }
    }

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
    let engine =
        crate::engine::factory::build(&app, &settings).map_err(|e| anyhow::anyhow!("{e}"))?;
    engine
        .health_check()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(true)
}

// ---------- Model management commands (Phase 5) ----------

/// Start downloading a GGML model by catalog id.
///
/// Spawns the download on the Tokio runtime. Progress is emitted as
/// `model:download_progress` events at ~10 Hz; completion emits
/// `model:download_complete`; failure emits `model:download_error`.
///
/// If a download for the same id is already in progress this is a no-op
/// (the existing download continues unaffected). The caller should check
/// the in-progress state via the frontend's `useModelDownloads` hook.
#[tauri::command]
pub async fn download_model<R: Runtime>(id: String, app: AppHandle<R>) -> AppResult<()> {
    use crate::app::AppState;
    use crate::events::{
        ModelDownloadCompletePayload, ModelDownloadErrorPayload, ModelDownloadProgressPayload,
        EV_MODEL_DOWNLOAD_COMPLETE, EV_MODEL_DOWNLOAD_ERROR, EV_MODEL_DOWNLOAD_PROGRESS,
    };
    use crate::models;

    // Guard: one download per model id.
    {
        let state = app
            .try_state::<AppState>()
            .ok_or_else(|| anyhow::anyhow!("AppState missing"))?;
        let mut tokens = match state.download_tokens.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if tokens.contains_key(&id) {
            // Already downloading — no-op.
            return Ok(());
        }
        let token = CancellationToken::new();
        tokens.insert(id.clone(), token);
    }

    let models_dir = models::models_dir(&app)
        .ok_or_else(|| anyhow::anyhow!("could not resolve app data dir"))?;

    // Clone what the task needs; AppHandle is cheap to clone (Arc inside).
    let app2 = app.clone();
    let id2 = id.clone();

    tokio::spawn(async move {
        let cancel = {
            let state = match app2.try_state::<AppState>() {
                Some(s) => s,
                None => {
                    tracing::warn!("AppState missing in download task");
                    return;
                }
            };
            let tokens = match state.download_tokens.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            match tokens.get(&id2) {
                Some(t) => t.clone(),
                None => return, // token was already removed (race with cancel)
            }
        };

        let app_clone = app2.clone();
        let id_clone = id2.clone();
        let result = crate::models::download::download(
            &id2,
            models_dir,
            cancel,
            move |bytes_done, bytes_total| {
                let _ = app_clone.emit(
                    EV_MODEL_DOWNLOAD_PROGRESS,
                    ModelDownloadProgressPayload {
                        id: id_clone.clone(),
                        bytes_done,
                        bytes_total,
                    },
                );
            },
        )
        .await;

        // Remove token regardless of outcome.
        if let Some(state) = app2.try_state::<AppState>() {
            let mut tokens = match state.download_tokens.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            tokens.remove(&id2);
        }

        match result {
            Ok(_path) => {
                // Sync installed_models in settings so the Engine tab picker
                // and the Models tab both see the new model immediately.
                if let Some(store) = app2.try_state::<crate::settings::SettingsStore>() {
                    let settings = store.get();
                    let mut installed = settings.local.installed_models.clone();
                    if !installed.contains(&id2) {
                        installed.push(id2.clone());
                    }
                    // Sort for stability.
                    installed.sort();
                    let patch = json!({"local": {"installed_models": installed}});
                    if let Ok(next) = store.update_patch(&app2, patch) {
                        let _ = app2.emit(EV_SETTINGS_UPDATED, next);
                    }
                }
                let _ = app2.emit(
                    EV_MODEL_DOWNLOAD_COMPLETE,
                    ModelDownloadCompletePayload { id: id2 },
                );
            }
            Err(e) => {
                // Cancelled is not an error worth surfacing.
                if !matches!(e, crate::models::download::DownloadError::Cancelled) {
                    let _ = app2.emit(
                        EV_MODEL_DOWNLOAD_ERROR,
                        ModelDownloadErrorPayload {
                            id: id2,
                            message: e.to_string(),
                        },
                    );
                }
            }
        }
    });

    Ok(())
}

/// Cancel an in-progress model download by id. No-op if none is running.
#[tauri::command]
pub async fn cancel_model_download<R: Runtime>(id: String, app: AppHandle<R>) -> AppResult<()> {
    use crate::app::AppState;

    if let Some(state) = app.try_state::<AppState>() {
        let tokens = match state.download_tokens.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(token) = tokens.get(&id) {
            token.cancel();
        }
    }
    Ok(())
}

/// Delete an installed GGML model file and sync `installed_models` in settings.
#[tauri::command]
pub async fn delete_model<R: Runtime>(
    id: String,
    app: AppHandle<R>,
    store: State<'_, crate::settings::SettingsStore>,
) -> AppResult<()> {
    use crate::events::EV_MODEL_DOWNLOAD_COMPLETE; // reuse event for refresh trigger
    use crate::models;

    let models_dir = models::models_dir(&app)
        .ok_or_else(|| anyhow::anyhow!("could not resolve app data dir"))?;

    models::download::delete(&id, &models_dir)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Sync installed_models in settings.
    let settings = store.get();
    let installed: Vec<String> = settings
        .local
        .installed_models
        .into_iter()
        .filter(|m| m != &id)
        .collect();

    // If active model was deleted, reset to first installed or leave as-is
    // (engine will return ModelNotInstalled on next dictation — handled there).
    let next = store
        .update_patch(&app, json!({"local": {"installed_models": installed}}))
        .context("sync installed_models after delete")?;

    // Clear engine cache so the next dictation doesn't try to use a gone file.
    if let Some(state) = app.try_state::<crate::app::AppState>() {
        state.clear_engine();
    }

    if let Err(e) = app.emit(EV_SETTINGS_UPDATED, next) {
        tracing::warn!(?e, "emit settings:updated after delete_model failed");
    }
    // Signal the frontend models tab to refresh.
    let _ = app.emit(
        EV_MODEL_DOWNLOAD_COMPLETE,
        crate::events::ModelDownloadCompletePayload { id },
    );

    Ok(())
}

/// Return the list of installed model ids by scanning disk and
/// cross-referencing with the catalog.
#[tauri::command]
pub async fn list_installed_models<R: Runtime>(app: AppHandle<R>) -> AppResult<Vec<String>> {
    use crate::models;

    let models_dir = models::models_dir(&app)
        .ok_or_else(|| anyhow::anyhow!("could not resolve app data dir"))?;

    Ok(models::download::list_installed(&models_dir).await)
}

/// Resolve a known app file path the frontend can pass back to `open_path`.
/// We expose it as a command rather than computing in the renderer because
/// `path::PathResolver` is Rust-only.
///
/// Returns the path to the log file (which may be rotating, e.g.
/// `wispertype.log.2026-05-07`). For "open the log directory" use
/// `open_log_dir` instead.
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

/// Open the log directory in Finder.
///
/// Phase 9: `tracing-appender` writes rotating daily files to this directory.
/// Revealing the directory (not one specific file) lets the user see all
/// dated log files without needing to know the current date suffix.
#[tauri::command]
pub async fn open_log_dir<R: Runtime>(app: AppHandle<R>) -> AppResult<()> {
    let home = app.path().home_dir().context("resolve home dir")?;
    let log_dir = home.join("Library/Logs/wispertype");
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        tracing::warn!(?e, "create log dir failed (non-fatal)");
    }
    let path_str = log_dir.to_string_lossy().into_owned();
    let status = std::process::Command::new("open")
        .arg(&path_str)
        .status()
        .with_context(|| format!("spawn `open` for {path_str}"))?;
    if !status.success() {
        return Err(anyhow::anyhow!("`open {path_str}` exited with {status}").into());
    }
    Ok(())
}

// ---------- Audio device management ----------

/// Information about an available input device.
#[derive(Debug, Serialize, Clone)]
pub struct InputDeviceInfo {
    /// Device identifier (device name on macOS). Passed back to `start_with_device`.
    pub id: String,
    /// Human-readable device name.
    pub name: String,
    /// True if this is the system default input device.
    pub is_default: bool,
}

/// List all available input audio devices.
///
/// Returns a vector of InputDeviceInfo structs, one per device. On macOS,
/// `id` and `name` are the same (cpal doesn't expose stable device IDs).
/// Sorting is unspecified; the frontend should sort by (is_default desc, name asc)
/// if desired.
#[tauri::command]
pub async fn list_input_devices() -> AppResult<Vec<InputDeviceInfo>> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();

    // Get the default device name for comparison.
    let default_name = host.default_input_device().and_then(|d| d.name().ok());

    // Enumerate all input devices.
    let devices = host
        .input_devices()
        .context("enumerate input devices")?
        .filter_map(|device| {
            let name = device.name().ok()?;
            let is_default = default_name.as_ref() == Some(&name);
            Some(InputDeviceInfo {
                id: name.clone(),
                name,
                is_default,
            })
        })
        .collect();

    Ok(devices)
}
