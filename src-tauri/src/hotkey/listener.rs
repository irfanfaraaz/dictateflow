//! Global-shortcut listener with live rebind + hold/toggle mode support.
//!
//! Phase 8 replaces the hard-coded F5 listener with a `HotkeyManager` that:
//!   * Parses the persisted `hotkey.binding` string into a `Shortcut`.
//!   * Dispatches `Pressed` / `Released` into `DictationController` using
//!     whichever mode (Hold or Toggle) is current in settings at event time.
//!   * Exposes `rebind()` to unregister the old shortcut and register a new
//!     one atomically — called from the `rebind_hotkey` Tauri command.
//!
//! ## Format contract
//! The frontend key-capture logic produces strings like `"Cmd+Shift+Space"` or
//! `"F5"`. The `global-hotkey` crate's `Shortcut::from_str` / `.parse()` is
//! case-insensitive on modifier tokens and accepts `CMD`, `SHIFT`, `ALT`,
//! `CTRL`, as well as key names like `SPACE`, `F5`, `A`, etc.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tracing::{info, warn};

use super::HotkeyError;
use crate::app::DictationController;
use crate::settings::schema::HotkeyMode;
use crate::settings::SettingsStore;

/// Manages the currently-registered global shortcut and exposes a `rebind`
/// method for live re-registration without restarting the app.
///
/// Only one shortcut is active at a time. The `current` field tracks which
/// `Shortcut` is registered so `rebind` can unregister it first.
pub struct HotkeyManager {
    current: Mutex<Option<Shortcut>>,
}

impl HotkeyManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            current: Mutex::new(None),
        })
    }

    /// Register `binding_str` (e.g. `"F5"` or `"Cmd+Shift+Space"`).
    ///
    /// Unregisters the previously-registered shortcut first. On parse failure
    /// or plugin error returns `Err` — the old binding is already gone in that
    /// case (best-effort), so callers should re-register the previous binding
    /// if they need to preserve it.
    pub fn rebind<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        binding_str: &str,
    ) -> Result<(), HotkeyError> {
        let new_shortcut = Shortcut::from_str(binding_str)
            .map_err(|e| HotkeyError::InvalidChord(binding_str.to_owned(), e.to_string()))?;

        // Unregister the old shortcut (best-effort; log but don't fail).
        {
            let mut guard = match self.current.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if let Some(old) = guard.take() {
                if let Err(e) = app.global_shortcut().unregister(old) {
                    warn!(?e, "failed to unregister old shortcut (continuing)");
                }
            }
        }

        // Build the handler closure. It reads mode dynamically from settings
        // so a mode-switch (hold → toggle) takes effect without a rebind.
        let controller = app
            .try_state::<Arc<DictationController>>()
            .map(|s| s.inner().clone())
            .ok_or_else(|| HotkeyError::Register("DictationController not in app state".into()))?;

        let app_for_handler = app.clone();

        app.global_shortcut()
            .on_shortcut(new_shortcut, move |_app, _sc, event| {
                // Read current mode without holding anything across the spawn.
                let mode: HotkeyMode = app_for_handler
                    .try_state::<SettingsStore>()
                    .map(|s| s.get().hotkey.mode)
                    .unwrap_or(HotkeyMode::Hold);

                match (mode, event.state()) {
                    // Hold mode: start on press, stop on release.
                    (HotkeyMode::Hold, ShortcutState::Pressed) => {
                        let app = app_for_handler.clone();
                        let ctrl = controller.clone();
                        tokio::spawn(async move {
                            ctrl.start(app).await;
                        });
                    }
                    (HotkeyMode::Hold, ShortcutState::Released) => {
                        let app = app_for_handler.clone();
                        let ctrl = controller.clone();
                        tokio::spawn(async move {
                            ctrl.stop(app).await;
                        });
                    }

                    // Toggle mode: press-to-start, press-again-to-stop.
                    // Released events are ignored in toggle mode.
                    (HotkeyMode::Toggle, ShortcutState::Pressed) => {
                        let app = app_for_handler.clone();
                        let ctrl = controller.clone();
                        tokio::spawn(async move {
                            ctrl.toggle(app).await;
                        });
                    }
                    (HotkeyMode::Toggle, ShortcutState::Released) => {
                        // intentionally ignored
                    }
                }
            })
            .map_err(|e| HotkeyError::Register(e.to_string()))?;

        // Record the newly-registered shortcut.
        {
            let mut guard = match self.current.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *guard = Some(new_shortcut);
        }

        info!(%binding_str, "registered hotkey");
        Ok(())
    }
}

/// Register the initial hotkey from settings at app startup.
///
/// Falls back to `"F5"` if the stored binding fails to parse or register, so
/// the app stays usable after a corrupt settings.json.
pub fn register_from_settings<R: Runtime>(app: &AppHandle<R>) -> Result<(), HotkeyError> {
    let binding = app
        .try_state::<SettingsStore>()
        .map(|s| s.get().hotkey.binding)
        .unwrap_or_else(|| "F5".into());

    let manager = app
        .try_state::<Arc<HotkeyManager>>()
        .map(|s| s.inner().clone())
        .ok_or_else(|| HotkeyError::Register("HotkeyManager not in app state".into()))?;

    if let Err(e) = manager.rebind(app, &binding) {
        warn!(?e, %binding, "failed to register configured hotkey, falling back to F5");
        manager.rebind(app, "F5")?;
    }

    Ok(())
}
