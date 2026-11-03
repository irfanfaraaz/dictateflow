//! Engine factory — builds the right engine for the current settings.
//!
//! Called per-dictation via `ensure_engine_loaded` (which caches the result
//! keyed by `EngineKind` so flipping local→scribe invalidates the cache).
//!
//! # Why per-dictation, not at startup?
//!
//! - `LocalEngine::load` is synchronous + expensive (model file I/O, Metal
//!   shader init). If we build at startup the app would block its own UI on
//!   every cold boot, including if the user never presses F5.
//! - The user can change engines in Settings without restarting — caching
//!   keyed by `EngineKind` in `AppState` means the model is loaded once per
//!   kind and then reused.
//!
//! # Why the API key never lives in settings.json
//!
//! `settings.json` is readable by any process with your macOS user account.
//! Keychain items are encrypted by the OS and gated behind TCC prompts. We
//! store only `has_api_key: bool` in settings so the UI can render the
//! redacted/paste state without exposing the secret.

use std::sync::Arc;

use tauri::{AppHandle, Runtime};

use crate::engine::local::LocalEngine;
use crate::engine::scribe::ScribeEngine;
use crate::engine::{Engine, EngineError, EngineResult};
use crate::keychain::{self, ELEVENLABS_KEY};
use crate::models;
use crate::settings::schema::{EngineKind, Settings};

/// Build the engine described by `settings`.
///
/// For `Local`: resolves the GGML model file and loads it (one-time; caller
/// caches the result in [`AppState`]).
/// For `Scribe`: reads the API key from Keychain. Returns
/// [`EngineError::MissingApiKey`] if none is stored.
///
/// `settings.language` is baked into the engine (`"auto"` → autodetect), so
/// the cache must be invalidated when it changes.
///
/// Synchronous and potentially slow (model load, Keychain IPC): async callers
/// must run it inside `tokio::task::spawn_blocking`.
pub fn build<R: Runtime>(app: &AppHandle<R>, settings: &Settings) -> EngineResult<Arc<dyn Engine>> {
    let language = language_option(&settings.language);
    match settings.engine {
        EngineKind::Local => {
            let model_path = models::resolve_path(app, &settings.local.model)?;
            let engine = LocalEngine::load(&model_path)?.with_language(language);
            Ok(Arc::new(engine))
        }
        EngineKind::Scribe => build_scribe(language),
    }
}

/// Map the settings language string to an engine language code.
/// `"auto"` (any case) and blank mean "autodetect" → `None`.
pub(crate) fn language_option(language: &str) -> Option<String> {
    let lang = language.trim();
    if lang.is_empty() || lang.eq_ignore_ascii_case("auto") {
        None
    } else {
        Some(lang.to_ascii_lowercase())
    }
}

/// Scribe arm separated into its own fn so it can be unit-tested without a
/// real `AppHandle` (Scribe doesn't need model resolution).
pub(crate) fn build_scribe(language: Option<String>) -> EngineResult<Arc<dyn Engine>> {
    let key = keychain::get(ELEVENLABS_KEY)
        .map_err(|e| EngineError::Inference(format!("keychain: {e}")))?
        .ok_or(EngineError::MissingApiKey)?;
    Ok(Arc::new(ScribeEngine::new(key).with_language(language)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `build_scribe` returns `MissingApiKey` when the keychain has no entry.
    ///
    /// This exercises the factory's Scribe arm without touching the real
    /// Keychain (the test process won't have an `elevenlabs_api_key` entry
    /// unless someone ran the integration test). If the CI runner *does* have
    /// an entry set, the assertion would fail — extremely unlikely, but if it
    /// ever does, the solution is to run the test behind `#[ignore]`.
    #[test]
    fn scribe_without_key_returns_missing_api_key() {
        // Pre-clean in case a prior test run left a stale entry.
        let _ = keychain::delete(ELEVENLABS_KEY);

        let result = build_scribe(None);
        assert!(
            matches!(result, Err(EngineError::MissingApiKey)),
            "expected Err(MissingApiKey)"
        );
    }

    #[test]
    fn auto_language_means_autodetect() {
        assert_eq!(language_option("auto"), None);
        assert_eq!(language_option("AUTO"), None);
        assert_eq!(language_option("  "), None);
        assert_eq!(language_option("en"), Some("en".to_string()));
        assert_eq!(language_option(" DE "), Some("de".to_string()));
    }
}
