//! Pill window manager.
//!
//! The pill is a transparent, borderless, always-on-top, click-through Tauri
//! window. Tauri sets up most of those flags via `tauri.conf.json`, but
//! click-through (so the user can click *through* the pill into the focused
//! app underneath) requires calling `setIgnoresMouseEvents:true` on the
//! underlying `NSWindow` directly — Tauri does not expose this.
//!
//! All `NSWindow` calls must happen on the macOS main thread. Tauri provides
//! `AppHandle::run_on_main_thread` for that.
//!
//! Coordinate spaces:
//! - AppKit (`NSEvent::mouseLocation`, `NSScreen::frame`) uses bottom-left
//!   origin: y=0 is the bottom of the primary display.
//! - Tauri's `Window::set_position` uses top-left origin: y=0 is the top
//!   of the primary display.
//!
//! Convert via `tauri_y = screen_height - appkit_y - window_height`.

#![allow(unsafe_code, dead_code)] // wired into AppState in Task 1.6

use anyhow::{anyhow, Context, Result};
use objc2::rc::Retained;
use objc2_app_kit::{NSScreen, NSWindow};
use objc2_foundation::MainThreadMarker;
use tauri::{AppHandle, LogicalPosition, Manager, PhysicalPosition, Runtime, WebviewWindow};
use tracing::warn;

/// The Tauri label of the pill window (declared in tauri.conf.json).
pub const PILL_LABEL: &str = "pill";

/// Width of the pill window in logical pixels (matches tauri.conf.json).
pub const PILL_WIDTH: f64 = 80.0;
/// Height of the pill window in logical pixels (matches tauri.conf.json).
pub const PILL_HEIGHT: f64 = 44.0;

/// Distance from the bottom edge of the screen (logical px).
const BOTTOM_MARGIN: f64 = 32.0;

/// Apply `setIgnoresMouseEvents:true` to the pill window so clicks pass
/// through to whatever's underneath it. MUST be called on the main thread.
///
/// # Safety
///
/// We obtain the raw `*mut NSWindow` pointer from Tauri and reinterpret it
/// as a `&NSWindow`. Tauri owns the lifetime of the window for as long as
/// the WebviewWindow exists; we never store the pointer or use it past this
/// call.
pub fn make_click_through<R: Runtime>(window: &WebviewWindow<R>) -> Result<()> {
    let ns_window_ptr = window
        .ns_window()
        .map_err(|e| anyhow!("ns_window() failed: {e}"))?;
    if ns_window_ptr.is_null() {
        return Err(anyhow!("ns_window() returned null"));
    }
    // SAFETY: Tauri guarantees this pointer points to a live NSWindow as
    // long as the WebviewWindow handle is valid. We borrow it for the
    // duration of this call and never store it.
    unsafe {
        let ns_window = &*(ns_window_ptr as *const NSWindow);
        ns_window.setIgnoresMouseEvents(true);
    }
    Ok(())
}

/// Show the pill window positioned near the cursor.
///
/// Must be called from the main thread (use [`AppHandle::run_on_main_thread`]
/// to dispatch from a background task).
pub fn show_at_cursor<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    let window = app
        .get_webview_window(PILL_LABEL)
        .ok_or_else(|| anyhow!("pill window '{PILL_LABEL}' not found"))?;

    // Compute target position in Tauri (top-left) logical coords.
    let position = compute_pill_position().context("compute pill position")?;
    window
        .set_position(position)
        .map_err(|e| anyhow!("set_position failed: {e}"))?;
    window
        .show()
        .map_err(|e| anyhow!("show pill failed: {e}"))?;
    // Ensure click-through is enabled (idempotent — safe to re-apply).
    if let Err(e) = make_click_through(&window) {
        warn!(?e, "failed to set click-through on pill window");
    }
    Ok(())
}

/// Hide the pill window.
pub fn hide<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    let window = app
        .get_webview_window(PILL_LABEL)
        .ok_or_else(|| anyhow!("pill window '{PILL_LABEL}' not found"))?;
    window
        .hide()
        .map_err(|e| anyhow!("hide pill failed: {e}"))?;
    Ok(())
}

/// Position the pill at bottom-center of the primary display (Wispr Flow style).
fn compute_pill_position() -> Result<tauri::Position> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| anyhow!("compute_pill_position called off main thread"))?;

    let screen: Option<Retained<NSScreen>> = NSScreen::mainScreen(mtm);
    let screen = screen.ok_or_else(|| anyhow!("no main screen"))?;
    let screen_frame = screen.frame();
    let screen_height = screen_frame.size.height;
    let screen_width = screen_frame.size.width;

    let x = (screen_width - PILL_WIDTH) / 2.0;
    let y = screen_height - PILL_HEIGHT - BOTTOM_MARGIN;

    Ok(tauri::Position::Logical(LogicalPosition::new(x, y)))
}

/// Convenience: dispatch [`show_at_cursor`] onto the macOS main thread.
pub fn show_at_cursor_async<R: Runtime>(app: AppHandle<R>) {
    let app2 = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        if let Err(e) = show_at_cursor(&app2) {
            warn!(?e, "show_at_cursor failed");
        }
    }) {
        warn!(?e, "run_on_main_thread dispatch failed");
    }
}

/// Convenience: dispatch [`hide`] onto the macOS main thread.
pub fn hide_async<R: Runtime>(app: AppHandle<R>) {
    let app2 = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        if let Err(e) = hide(&app2) {
            warn!(?e, "hide pill failed");
        }
    }) {
        warn!(?e, "run_on_main_thread dispatch failed");
    }
}

// PhysicalPosition import is kept for potential future use by callers that
// want to bypass scale factor handling. Currently unused.
#[allow(dead_code)]
const _PHYS_KEEPALIVE: Option<PhysicalPosition<i32>> = None;
