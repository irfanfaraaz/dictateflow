//! Global-shortcut listener with live rebind + hold/toggle mode support.
//!
//! `HotkeyManager`:
//!   * Validates + parses the persisted `hotkey.binding` string into a
//!     `Shortcut` (see [`super::validate_binding`]).
//!   * Forwards every `Pressed` / `Released` event into ONE unbounded channel
//!     consumed by a single long-lived dispatcher task. The dispatcher awaits
//!     `start` / `stop` / `toggle` on the `DictationController` sequentially,
//!     so a release can never overtake the press that preceded it (the old
//!     spawn-per-event design let `stop()` run while `start()` was still
//!     building the session, leaving the mic live).
//!   * Exposes `rebind()`, which registers the new shortcut *before*
//!     unregistering the old one, so a failed rebind leaves the previous
//!     hotkey working.
//!
//! ## Format contract
//! The frontend key-capture logic produces strings like `"Cmd+Shift+Space"` or
//! `"F5"`. The `global-hotkey` crate's `Shortcut::from_str` is
//! case-insensitive and accepts `CMD`/`COMMAND`/`SUPER`, `SHIFT`,
//! `ALT`/`OPTION`, `CTRL`/`CONTROL`, and key names like `SPACE`, `F5`, `A`.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::{same_binding, validate_binding, HotkeyError, DEFAULT_BINDING};
use crate::app::DictationController;
use crate::settings::schema::HotkeyMode;
use crate::settings::SettingsStore;

/// Manages the currently-registered global shortcut and exposes a `rebind`
/// method for live re-registration without restarting the app.
///
/// Only one shortcut is active at a time (briefly two during a rebind).
pub struct HotkeyManager {
    current: Mutex<Option<Shortcut>>,
    /// Sender side of the dispatcher channel. Created lazily on the first
    /// registration (the dispatcher needs an `AppHandle`).
    dispatch: Mutex<Option<mpsc::UnboundedSender<ShortcutState>>>,
}

impl HotkeyManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            current: Mutex::new(None),
            dispatch: Mutex::new(None),
        })
    }

    /// Register `binding_str` (e.g. `"F5"` or `"Cmd+Shift+Space"`) as the
    /// dictation hotkey.
    ///
    /// The new shortcut is registered first; the old one is unregistered only
    /// after that succeeds. On any error the previous binding stays active.
    /// Re-binding to the shortcut that is already registered is a no-op.
    pub fn rebind<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        binding_str: &str,
    ) -> Result<(), HotkeyError> {
        let new_shortcut = validate_binding(binding_str)?;

        let already = {
            let guard = match self.current.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            guard
                .as_ref()
                .is_some_and(|cur| same_binding(cur, &new_shortcut))
        };
        if already {
            debug!(%binding_str, "hotkey unchanged — rebind is a no-op");
            return Ok(());
        }

        let tx = self.dispatcher(app)?;
        app.global_shortcut()
            .on_shortcut(new_shortcut, move |_app, _sc, event| {
                // Never block or spawn here: just enqueue. The dispatcher
                // task applies the events in order.
                if tx.send(event.state()).is_err() {
                    warn!("hotkey dispatcher gone — dropping shortcut event");
                }
            })
            .map_err(|e| HotkeyError::Register(e.to_string()))?;

        // New shortcut is live — now retire the old one.
        let old = {
            let mut guard = match self.current.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            guard.replace(new_shortcut)
        };
        if let Some(old) = old {
            if let Err(e) = app.global_shortcut().unregister(old) {
                warn!(?e, "failed to unregister previous shortcut (continuing)");
            }
        }

        info!(%binding_str, "registered hotkey");
        Ok(())
    }

    /// Return the dispatcher sender, spawning the dispatcher task on first use.
    fn dispatcher<R: Runtime>(
        &self,
        app: &AppHandle<R>,
    ) -> Result<mpsc::UnboundedSender<ShortcutState>, HotkeyError> {
        let mut guard = match self.dispatch.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(tx) = guard.as_ref() {
            if !tx.is_closed() {
                return Ok(tx.clone());
            }
        }
        let controller = app
            .try_state::<Arc<DictationController>>()
            .map(|s| s.inner().clone())
            .ok_or_else(|| HotkeyError::Register("DictationController not in app state".into()))?;
        let (tx, rx) = mpsc::unbounded_channel();
        tauri::async_runtime::spawn(run_dispatcher(app.clone(), controller, rx));
        *guard = Some(tx.clone());
        Ok(tx)
    }
}

/// Single consumer of hotkey events. Each controller call is awaited before
/// the next event is handled, so start/stop/toggle never interleave.
async fn run_dispatcher<R: Runtime>(
    app: AppHandle<R>,
    controller: Arc<DictationController>,
    mut rx: mpsc::UnboundedReceiver<ShortcutState>,
) {
    // macOS key-repeat can deliver several `Pressed` events for one physical
    // press; only the first one after a `Released` counts.
    let mut key_down = false;
    while let Some(state) = rx.recv().await {
        // Read mode at event time so a hold → toggle switch in Settings takes
        // effect without a rebind.
        let mode: HotkeyMode = app
            .try_state::<SettingsStore>()
            .map(|s| s.get().hotkey.mode)
            .unwrap_or(HotkeyMode::Hold);

        match state {
            ShortcutState::Pressed => {
                if key_down {
                    debug!("ignoring repeated Pressed (key repeat)");
                    continue;
                }
                key_down = true;
                match mode {
                    HotkeyMode::Hold => controller.start(&app).await,
                    HotkeyMode::Toggle => controller.toggle(&app).await,
                }
            }
            ShortcutState::Released => {
                key_down = false;
                match mode {
                    HotkeyMode::Hold => controller.stop(&app).await,
                    // Toggle mode acts on presses only.
                    HotkeyMode::Toggle => {}
                }
            }
        }
    }
    info!("hotkey dispatcher exiting (channel closed)");
}

/// Register the initial hotkey from settings at app startup.
///
/// The stored binding is validated like a user-entered one; if it is invalid
/// (corrupt settings.json, or a chord saved before validation tightened) or
/// fails to register, we fall back to [`DEFAULT_BINDING`] so the app stays
/// usable.
pub fn register_from_settings<R: Runtime>(app: &AppHandle<R>) -> Result<(), HotkeyError> {
    let stored = app
        .try_state::<SettingsStore>()
        .map(|s| s.get().hotkey.binding)
        .unwrap_or_else(|| DEFAULT_BINDING.into());

    let manager = app
        .try_state::<Arc<HotkeyManager>>()
        .map(|s| s.inner().clone())
        .ok_or_else(|| HotkeyError::Register("HotkeyManager not in app state".into()))?;

    let binding = match validate_binding(&stored) {
        Ok(_) => stored,
        Err(e) => {
            warn!(?e, %stored, "stored hotkey is invalid, falling back to {DEFAULT_BINDING}");
            DEFAULT_BINDING.to_owned()
        }
    };

    if let Err(e) = manager.rebind(app, &binding) {
        if binding == DEFAULT_BINDING {
            return Err(e);
        }
        warn!(?e, %binding, "failed to register configured hotkey, falling back to {DEFAULT_BINDING}");
        manager.rebind(app, DEFAULT_BINDING)?;
    }

    Ok(())
}
