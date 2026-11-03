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
use serde_json::Value;
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

/// Snapshot of all three TCC permissions DictateFlow cares about.
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

    let invalidates_engine = patch_invalidates_engine(&patch);
    let next = store.update_patch(&app, patch).context("update settings")?;

    // Engine kind, local model selection or language changed → drop the
    // cached engine so the next dictation builds one from the new settings.
    if invalidates_engine {
        clear_engine_cache(&app);
    }

    // Emit AFTER the store mutation so listeners on other windows can rely
    // on the snapshot reflecting disk state. Failures here are non-fatal —
    // the change is durable; we just lost the cross-window heads-up.
    if let Err(e) = app.emit(EV_SETTINGS_UPDATED, next.clone()) {
        tracing::warn!(?e, "emit settings:updated failed");
    }

    Ok(next)
}

/// Top-level settings keys whose change requires rebuilding the engine.
const ENGINE_AFFECTING_KEYS: &[&str] = &["engine", "local", "language"];

fn patch_invalidates_engine(patch: &Value) -> bool {
    patch
        .as_object()
        .is_some_and(|o| ENGINE_AFFECTING_KEYS.iter().any(|k| o.contains_key(*k)))
}

fn clear_engine_cache<R: Runtime>(app: &AppHandle<R>) {
    if let Some(state) = app.try_state::<crate::app::AppState>() {
        state.clear_engine();
    }
}

fn emit_settings<R: Runtime>(app: &AppHandle<R>, next: &Settings) {
    if let Err(e) = app.emit(EV_SETTINGS_UPDATED, next.clone()) {
        tracing::warn!(?e, "emit settings:updated failed");
    }
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

/// Open an allow-listed URL or app-owned path in the user's default handler.
/// Used by the About tab (releases page, log file).
///
/// The webview is not trusted to open arbitrary targets: only
/// - `https://github.com/irfanfaraaz/dictateflow[/...]`
/// - `https://elevenlabs.io[/...]` (and its subdomains)
/// - `x-apple.systempreferences:` deep links
/// - existing paths inside the app log dir or the models dir
///
/// are accepted; anything else is rejected with an error.
#[tauri::command]
pub async fn open_path<R: Runtime>(app: AppHandle<R>, path: String) -> AppResult<()> {
    let mut allowed_dirs = Vec::new();
    if let Ok(d) = log_dir(&app) {
        allowed_dirs.push(d);
    }
    if let Some(d) = crate::models::models_dir(&app) {
        allowed_dirs.push(d);
    }
    if !open_target_allowed(&path, &allowed_dirs) {
        tracing::warn!(target = %path, "open_path rejected non-allow-listed target");
        return Err(anyhow::anyhow!("refusing to open {path}: not an allowed location").into());
    }

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

/// Allow-list check for [`open_path`]. Absolute paths must exist and resolve
/// (after symlinks / `..`) inside one of `allowed_dirs`; everything else must
/// be an allow-listed URL.
fn open_target_allowed(target: &str, allowed_dirs: &[std::path::PathBuf]) -> bool {
    if target.starts_with('/') {
        let Ok(resolved) = std::fs::canonicalize(target) else {
            return false;
        };
        return allowed_dirs.iter().any(|dir| {
            std::fs::canonicalize(dir)
                .map(|d| resolved.starts_with(d))
                .unwrap_or(false)
        });
    }
    url_allowed(target)
}

fn url_allowed(target: &str) -> bool {
    let Ok(url) = url::Url::parse(target) else {
        return false;
    };
    match url.scheme() {
        "x-apple.systempreferences" => true,
        "https" => {
            if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
                return false;
            }
            match url.host_str() {
                Some("github.com") => {
                    let p = url.path();
                    p == "/irfanfaraaz/dictateflow" || p.starts_with("/irfanfaraaz/dictateflow/")
                }
                Some(h) => h == "elevenlabs.io" || h.ends_with(".elevenlabs.io"),
                None => false,
            }
        }
        _ => false,
    }
}

/// `~/Library/Logs/dictateflow` — where `tracing-appender` writes.
fn log_dir<R: Runtime>(app: &AppHandle<R>) -> anyhow::Result<std::path::PathBuf> {
    let home = app.path().home_dir().context("resolve home dir")?;
    Ok(home.join("Library/Logs/dictateflow"))
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
    // Storing a key is a "setup done" moment: if Scribe is the selected
    // engine the app is now usable, so later launches can start in the tray.
    let next = store
        .update_with(&app, |s| {
            s.scribe.has_api_key = true;
            if s.engine == crate::settings::schema::EngineKind::Scribe {
                s.first_run_completed = true;
            }
        })
        .context("update settings after set_api_key")?;

    // Clear any cached Scribe engine so the next dictation picks up the new key.
    clear_engine_cache(&app);

    emit_settings(&app, &next);
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
        .update_with(&app, |s| s.scribe.has_api_key = false)
        .context("update settings after clear_api_key")?;

    // Evict cached Scribe engine.
    clear_engine_cache(&app);

    emit_settings(&app, &next);
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

    // Resolve everything fallible BEFORE claiming the per-id slot, so an
    // early error can't leave a token behind (which would turn every later
    // download of this id into a silent no-op).
    let models_dir = models::models_dir(&app)
        .ok_or_else(|| anyhow::anyhow!("could not resolve app data dir"))?;

    // Guard: one download per model id.
    let cancel = {
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
        tokens.insert(id.clone(), token.clone());
        token
    };
    // From here on the slot is released on every exit path (including a
    // panic inside the task) when the guard drops.
    let slot = DownloadSlot {
        app: app.clone(),
        id: id.clone(),
    };

    // Clone what the task needs; AppHandle is cheap to clone (Arc inside).
    let app2 = app.clone();
    let id2 = id.clone();

    tokio::spawn(async move {
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

        // Release the per-id slot before notifying the UI, so a retry
        // triggered by the event isn't rejected as "already downloading".
        drop(slot);

        match result {
            Ok(_path) => {
                // Sync installed_models in settings so the Engine tab picker
                // and the Models tab both see the new model immediately.
                // Atomic read-modify-write: concurrent completions can't
                // drop each other's ids.
                if let Some(store) = app2.try_state::<crate::settings::SettingsStore>() {
                    let mut model_changed = false;
                    let res = store.update_with(&app2, |s| {
                        let installed = &mut s.local.installed_models;
                        if !installed.contains(&id2) {
                            installed.push(id2.clone());
                        }
                        // Sort for stability.
                        installed.sort();
                        // If the active model isn't actually installed (e.g.
                        // default "small" while the user fetched "tiny"),
                        // switch to the one that just arrived.
                        if !s.local.installed_models.contains(&s.local.model) {
                            s.local.model = id2.clone();
                            model_changed = true;
                        }
                        if s.engine == crate::settings::schema::EngineKind::Local {
                            s.first_run_completed = true;
                        }
                    });
                    match res {
                        Ok(next) => {
                            if model_changed {
                                clear_engine_cache(&app2);
                            }
                            emit_settings(&app2, &next);
                        }
                        Err(e) => {
                            tracing::error!(?e, id = %id2, "persist installed_models failed");
                        }
                    }
                }
                let _ = app2.emit(
                    EV_MODEL_DOWNLOAD_COMPLETE,
                    ModelDownloadCompletePayload { id: id2 },
                );
            }
            Err(e) => {
                // Cancellation is reported with the sentinel message
                // "cancelled" so the frontend can reset its progress UI
                // without showing an error.
                let message = if matches!(e, crate::models::download::DownloadError::Cancelled) {
                    "cancelled".to_string()
                } else {
                    tracing::warn!(error = %e, id = %id2, "model download failed");
                    e.to_string()
                };
                let _ = app2.emit(
                    EV_MODEL_DOWNLOAD_ERROR,
                    ModelDownloadErrorPayload { id: id2, message },
                );
            }
        }
    });

    Ok(())
}

/// Scope guard for a claimed `download_tokens` slot: removes the id's token
/// when dropped, whatever path the download took.
struct DownloadSlot<R: Runtime> {
    app: AppHandle<R>,
    id: String,
}

impl<R: Runtime> Drop for DownloadSlot<R> {
    fn drop(&mut self) {
        if let Some(state) = self.app.try_state::<crate::app::AppState>() {
            let mut tokens = match state.download_tokens.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            tokens.remove(&self.id);
        }
    }
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

    // Sync installed_models in settings (atomic read-modify-write). If the
    // active model was deleted, switch to the best remaining installed model
    // (if none remain, leave it — the engine reports ModelNotInstalled and
    // the UI prompts for a download).
    let next = store
        .update_with(&app, |s| {
            s.local.installed_models.retain(|m| m != &id);
            if s.local.model == id {
                if let Some(best) = models::catalog::best_installed(&s.local.installed_models) {
                    s.local.model = best.to_string();
                }
            }
        })
        .context("sync installed_models after delete")?;

    // Clear engine cache so the next dictation doesn't try to use a gone file.
    clear_engine_cache(&app);

    emit_settings(&app, &next);
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
/// `dictateflow.log.2026-05-07`). For "open the log directory" use
/// `open_log_dir` instead.
#[tauri::command]
pub async fn resolve_log_path<R: Runtime>(app: AppHandle<R>) -> AppResult<String> {
    // `~/Library/Logs/dictateflow/dictateflow.log`. The directory is created
    // on first call so `open` doesn't fail with ENOENT before Phase 9 wires
    // the writer.
    let log_dir = log_dir(&app)?;
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        tracing::warn!(?e, "create log dir failed (non-fatal)");
    }
    let log_file = log_dir.join("dictateflow.log");
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
    let log_dir = log_dir(&app)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn allowed_urls() {
        assert!(url_allowed(
            "https://github.com/irfanfaraaz/dictateflow/releases"
        ));
        assert!(url_allowed("https://github.com/irfanfaraaz/dictateflow"));
        assert!(url_allowed("https://elevenlabs.io/app/settings/api-keys"));
        assert!(url_allowed(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        ));
    }

    #[test]
    fn rejected_urls() {
        assert!(!url_allowed(
            "https://github.com/irfanfaraaz/dictateflow-evil"
        ));
        assert!(!url_allowed("https://github.com/someone/else"));
        assert!(!url_allowed("http://github.com/irfanfaraaz/dictateflow"));
        assert!(!url_allowed("https://elevenlabs.io.evil.com/"));
        assert!(!url_allowed("https://user@elevenlabs.io/"));
        assert!(!url_allowed("file:///etc/passwd"));
        assert!(!url_allowed("javascript:alert(1)"));
        assert!(!url_allowed("/Applications/Calculator.app"));
        assert!(!url_allowed("-a Terminal"));
    }

    #[test]
    fn paths_must_be_inside_allowed_dirs() {
        let base = std::env::temp_dir().join(format!("df-open-test-{}", std::process::id()));
        let allowed = base.join("logs");
        let other = base.join("other");
        std::fs::create_dir_all(&allowed).expect("mkdir allowed");
        std::fs::create_dir_all(&other).expect("mkdir other");
        let inside = allowed.join("dictateflow.log");
        let outside = other.join("x.log");
        std::fs::write(&inside, "").expect("write inside");
        std::fs::write(&outside, "").expect("write outside");
        let dirs = [allowed.clone()];

        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        assert!(open_target_allowed(&s(&inside), &dirs));
        assert!(open_target_allowed(&s(&allowed), &dirs));
        assert!(!open_target_allowed(&s(&outside), &dirs));
        // `..` traversal is resolved before the prefix check.
        let sneaky = format!("{}/../other/x.log", s(&allowed));
        assert!(!open_target_allowed(&sneaky, &dirs));
        // Non-existent paths are rejected.
        assert!(!open_target_allowed(&s(&allowed.join("nope")), &dirs));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn engine_invalidation_keys() {
        assert!(patch_invalidates_engine(&json!({"engine": "scribe"})));
        assert!(patch_invalidates_engine(
            &json!({"local": {"model": "tiny"}})
        ));
        assert!(patch_invalidates_engine(&json!({"language": "de"})));
        assert!(!patch_invalidates_engine(
            &json!({"pill": {"follow_cursor": false}})
        ));
        assert!(!patch_invalidates_engine(&json!("nonsense")));
    }
}
