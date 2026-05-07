//! F5 hold-to-talk listener.
//!
//! Phase 1 hard-codes F5 with no modifiers. Phase 8 introduces user-rebindable
//! chord capture. The listener calls into the [`DictationController`] which
//! owns the audio + pill lifecycle — this module knows nothing about audio
//! plumbing.

use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};
use tracing::{info, warn};

use super::HotkeyError;
use crate::app::DictationController;

/// The Phase 1 placeholder hotkey: F5 with no modifiers.
pub fn f5_shortcut() -> Shortcut {
    Shortcut::new(None, Code::F5)
}

/// Register F5 with the global-shortcut plugin and dispatch press/release
/// into the [`DictationController`].
///
/// Idempotent across a single app launch; the plugin returns an error if you
/// register the same shortcut twice, which we treat as a soft failure.
pub fn register<R: Runtime>(app: &AppHandle<R>) -> Result<(), HotkeyError> {
    let controller = app
        .try_state::<Arc<DictationController>>()
        .map(|s| s.inner().clone())
        .ok_or_else(|| HotkeyError::Register("DictationController not in app state".into()))?;

    let shortcut = f5_shortcut();
    let app_for_handler = app.clone();

    app.global_shortcut()
        .on_shortcut(shortcut, move |_app, _sc, event| match event.state() {
            ShortcutState::Pressed => {
                let app = app_for_handler.clone();
                let controller = controller.clone();
                tokio::spawn(async move {
                    controller.start(app).await;
                });
            }
            ShortcutState::Released => {
                let app = app_for_handler.clone();
                let controller = controller.clone();
                tokio::spawn(async move {
                    controller.stop(app).await;
                });
            }
        })
        .map_err(|e| HotkeyError::Register(e.to_string()))?;

    info!("registered F5 hold-to-talk hotkey");
    Ok(())
}

/// Best-effort unregister; logs but never panics on failure.
pub fn unregister<R: Runtime>(app: &AppHandle<R>) {
    if let Err(e) = app.global_shortcut().unregister(f5_shortcut()) {
        warn!(?e, "failed to unregister F5");
    }
}
