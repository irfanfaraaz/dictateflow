//! GGML model resolution + download management.
//!
//! Phase 2 only needed to *find* the bundled `ggml-small.bin` (or one a
//! developer dropped into the app data dir). Phase 5 grows this module into
//! a full downloader + checksum verifier + catalog with multiple model sizes.
//!
//! Public surface:
//! - `catalog` — static model metadata (ids, URLs, SHA-256, sizes).
//! - `download` — streaming downloader, SHA-256 verifier, file management.
//! - `resolve_path` — locate an installed model by id (app data dir only
//!   after Phase 5 removes the bundled resource).

pub mod catalog;
pub mod download;

use std::path::PathBuf;

use tauri::{AppHandle, Manager, Runtime};

use crate::engine::EngineError;

/// Resolve the on-disk path for a GGML model id.
///
/// Lookup order (first hit wins):
///
/// 1. **App data dir** — `~/Library/Application Support/com.wispertype.app/models/ggml-<id>.bin`.
///    This is where Phase 5's UI downloader writes new models, and where a
///    dev can drop a file via `scripts/download-model.sh` without rebuilding.
/// 2. **Bundled resource** — `<resource_dir>/ggml-<id>.bin`. Tauri's
///    `PathResolver` returns the location next to the binary inside the
///    `.app` bundle (or the dev `target/` directory during `tauri dev`).
///    Left in for development convenience; not used in production after Phase 5.
///
/// Returns [`EngineError::ModelNotInstalled`] if neither location has the
/// file.
pub fn resolve_path<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<PathBuf, EngineError> {
    let file_name = format!("ggml-{id}.bin");

    // 1. App data dir (writable, hot-swappable).
    if let Ok(data_dir) = app.path().app_data_dir() {
        let candidate = data_dir.join("models").join(&file_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    // 2. Bundled resource (read-only, ships with the .app).
    if let Ok(resource_dir) = app.path().resource_dir() {
        let candidate = resource_dir.join("resources").join(&file_name);
        if candidate.exists() {
            return Ok(candidate);
        }
        // Some Tauri layouts flatten `resources/` into the root resource dir.
        let alt = resource_dir.join(&file_name);
        if alt.exists() {
            return Ok(alt);
        }
    }

    Err(EngineError::ModelNotInstalled(file_name))
}

/// Returns the `models/` subdirectory inside the app data dir.
///
/// This is where the downloader writes files. Does NOT create the directory —
/// `download::download()` handles that.
pub fn models_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("models"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_model_id_renders_filename_in_error() {
        // We can't easily synthesise a Tauri AppHandle in a unit test, so we
        // exercise the error formatting directly.
        let err = EngineError::ModelNotInstalled("ggml-small.bin".to_string());
        let msg = err.to_string();
        assert!(msg.contains("ggml-small.bin"), "got: {msg}");
    }
}
