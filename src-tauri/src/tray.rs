//! Menu-bar tray icon with state-driven variants.
//!
//! # Icon assets
//! Three logical states — Idle, Listening, Error — should map to three
//! distinct 22×22px PNGs at:
//!
//!   src-tauri/icons/tray-idle.png
//!   src-tauri/icons/tray-listening.png
//!   src-tauri/icons/tray-error.png
//!
//! TODO(phase-10): source distinct assets from the design pass. Until then
//! all three states share `icons/icon.png` so the code structure is in place
//! without a blank/missing-asset panic.
//!
//! # Collision with tauri.conf.json
//! Remove (or leave absent) the `"trayIcon"` key from `tauri.conf.json`.
//! That static config path auto-creates a second icon on startup. We create
//! the icon programmatically here so state transitions work via `set_icon`.

use std::sync::atomic::{AtomicU64, Ordering};

use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};
use tracing::{debug, warn};

/// Logical state of the menu bar icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Listening,
    Error,
}

/// Generation counter used to cancel stale "revert-to-idle" tasks spawned
/// when entering the Error state. Increment before spawning; if the counter
/// differs when the sleep wakes, a newer transition has happened — bail.
static ERROR_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Create the tray icon and menu on startup.
///
/// Called from `lib::run` during `.setup()`. Returns the `TrayIcon` handle;
/// Tauri keeps it alive as part of the app — the caller can ignore the
/// return value.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<TrayIcon<R>> {
    let open_item = MenuItem::with_id(app, "open_settings", "Open Settings", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_item, &quit_item])?;

    let icon = load_icon(app, TrayState::Idle);

    let tray = TrayIconBuilder::new()
        .icon(icon)
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(false) // we handle left-click ourselves below
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                show_settings_window(app);
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open_settings" => show_settings_window(app),
            "quit" => {
                debug!("quit from tray menu");
                app.exit(0);
            }
            other => {
                warn!(id = other, "unknown tray menu item");
            }
        })
        .build(app)?;

    Ok(tray)
}

/// Update the tray icon to reflect the current dictation state.
///
/// The Error state automatically reverts to Idle after ~3 seconds using a
/// generation counter to suppress stale writes from prior error events.
pub fn set_state<R: Runtime>(app: &AppHandle<R>, state: TrayState) -> tauri::Result<()> {
    // Resolve the tray icon. Tauri 2 stores it under a default id ("" or the
    // first registered tray). We use `tray_by_id` with an empty string which
    // is what `TrayIconBuilder::new()` (no explicit `.id()`) produces.
    let Some(tray) = app.tray_by_id("") else {
        warn!("set_state: tray icon not found — was build() called?");
        return Ok(());
    };

    let icon = load_icon(app, state);
    tray.set_icon(Some(icon))?;
    debug!(?state, "tray state updated");

    // For Error: schedule an auto-revert to Idle after 3 s.
    if state == TrayState::Error {
        let gen = ERROR_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            // If generation has advanced, a newer transition happened — skip.
            if ERROR_GENERATION.load(Ordering::SeqCst) != gen {
                return;
            }
            if let Err(e) = set_state(&app2, TrayState::Idle) {
                warn!(?e, "tray auto-revert to idle failed");
            }
        });
    }

    Ok(())
}

// ---------- helpers ----------

/// Load the icon image for the given state.
///
/// TODO(phase-10): swap per-state assets once distinct icons are sourced from
/// the design pass. All three currently resolve to the bundled window icon
/// that Tauri already loaded from `bundle.icon` at startup — no decoding,
/// no resource_dir lookup, no panic surface.
fn load_icon<R: Runtime>(app: &AppHandle<R>, state: TrayState) -> Image<'static> {
    let _state_label = match state {
        TrayState::Idle => "idle",
        TrayState::Listening => "listening",
        TrayState::Error => "error",
    };

    if let Some(icon) = app.default_window_icon() {
        return Image::new_owned(icon.rgba().to_vec(), icon.width(), icon.height());
    }

    warn!("default_window_icon unavailable — using 1x1 transparent fallback");
    // 1x1 transparent RGBA pixel via the raw constructor (no PNG decode).
    Image::new_owned(vec![0, 0, 0, 0], 1, 1)
}

/// Show + focus the settings window (tray menu, tray click, Dock reopen,
/// second launch, first-run onboarding).
pub fn show_settings_window<R: Runtime>(app: &AppHandle<R>) {
    match app.get_webview_window("settings") {
        Some(win) => {
            if let Err(e) = win.show() {
                warn!(?e, "show settings window failed");
            }
            if let Err(e) = win.unminimize() {
                debug!(?e, "unminimize settings window failed");
            }
            if let Err(e) = win.set_focus() {
                debug!(?e, "focus settings window failed");
            }
        }
        None => warn!("settings window missing"),
    }
}
