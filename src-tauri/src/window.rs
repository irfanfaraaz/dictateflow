//! Pill window manager.
//!
//! The pill is a transparent, borderless, always-on-top, click-through Tauri
//! window. Tauri sets most of those flags via `tauri.conf.json`; the rest
//! (click-through, window level, Spaces behaviour, positioning) are applied
//! directly on the underlying `NSWindow` because Tauri doesn't expose them.
//!
//! All `NSWindow` / `NSScreen` calls must happen on the macOS main thread.
//!
//! # Multi-monitor positioning
//!
//! We position the pill in AppKit's *global* coordinate space: points, with
//! the origin at the bottom-left of the primary display, spanning every
//! connected screen. Each `NSScreen::frame()` is expressed in that space, so
//! a monitor to the right of the laptop has `frame.origin.x > 0`, one to the
//! left has a negative x, one above has `origin.y > laptop height`.
//!
//! The previous implementation took only the *size* of `mainScreen()`,
//! ignored its origin, and converted to Tauri's top-left logical coordinates
//! using the height of the wrong screen. On an external monitor the pill was
//! placed relative to the primary display — usually off-screen entirely —
//! and Tauri's logical→physical conversion used the scale factor of whatever
//! monitor the window was last on. Setting the origin directly via
//! `setFrameOrigin` sidesteps both problems: global points are scale-free.

#![allow(unsafe_code)]

use anyhow::{anyhow, Result};
use objc2_app_kit::{NSEvent, NSScreen, NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect};
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};
use tracing::{debug, warn};

/// The Tauri label of the pill window (declared in tauri.conf.json).
pub const PILL_LABEL: &str = "pill";

/// Width of the pill window in points (matches tauri.conf.json).
/// Larger than the visible pill (~100×38) so its shadow and spring-in
/// overshoot are never clipped by the window bounds.
pub const PILL_WIDTH: f64 = 160.0;
/// Height of the pill window in points (matches tauri.conf.json).
pub const PILL_HEIGHT: f64 = 64.0;

/// Gap between the bottom of the screen's *visible* frame (i.e. above the
/// Dock, if it's at the bottom) and the pill window.
const BOTTOM_MARGIN: f64 = 20.0;

/// Axis-aligned rectangle in AppKit global coordinates (bottom-left origin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

impl From<NSRect> for Rect {
    fn from(r: NSRect) -> Self {
        Self {
            x: r.origin.x,
            y: r.origin.y,
            w: r.size.width,
            h: r.size.height,
        }
    }
}

/// One connected display: full frame plus the visible frame (excludes the
/// menu bar and the Dock).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenGeom {
    pub frame: Rect,
    pub visible: Rect,
}

/// Choose where the pill window's bottom-left corner goes.
///
/// Picks the screen under the mouse cursor (the one the user is working on),
/// falling back to the first screen (the primary display). Centers the pill
/// horizontally on that screen's visible frame and sits it just above the
/// Dock. Coordinates are rounded to whole points so the pill isn't blurry on
/// non-Retina displays.
pub fn pill_origin(screens: &[ScreenGeom], mouse: (f64, f64)) -> Option<(f64, f64)> {
    let screen = screens
        .iter()
        .find(|s| s.frame.contains(mouse.0, mouse.1))
        .or_else(|| screens.first())?;
    let v = screen.visible;
    let x = (v.x + (v.w - PILL_WIDTH) / 2.0).round();
    let y = (v.y + BOTTOM_MARGIN).round();
    Some((x, y))
}

/// Borrow the `NSWindow` behind a Tauri window.
fn ns_window_ref<'a, R: Runtime>(window: &'a WebviewWindow<R>) -> Result<&'a NSWindow> {
    let ptr = window
        .ns_window()
        .map_err(|e| anyhow!("ns_window() failed: {e}"))?;
    if ptr.is_null() {
        return Err(anyhow!("ns_window() returned null"));
    }
    // SAFETY: Tauri guarantees this pointer refers to a live NSWindow for as
    // long as the WebviewWindow handle is alive; the returned reference is
    // bounded by the borrow of `window`.
    Ok(unsafe { &*(ptr as *const NSWindow) })
}

/// Make the window behave like a system overlay. Idempotent; cheap enough to
/// re-apply on every show.
///
/// - click-through, so clicks land in the app underneath;
/// - status-bar window level, so it floats above normal windows and the Dock;
/// - visible on every Space and alongside full-screen apps (without this the
///   pill stays on the Space where it was created and is invisible from a
///   full-screen app or another desktop);
/// - excluded from ⌘` window cycling; never hides when the app deactivates.
fn configure_overlay(ns_window: &NSWindow) {
    ns_window.setIgnoresMouseEvents(true);
    ns_window.setLevel(NSStatusWindowLevel);
    let behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::Stationary
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::IgnoresCycle;
    // SAFETY: plain property setters on a live NSWindow, on the main thread.
    unsafe {
        ns_window.setCollectionBehavior(behavior);
        ns_window.setHidesOnDeactivate(false);
    }
}

fn connected_screens(mtm: MainThreadMarker) -> Vec<ScreenGeom> {
    let screens = NSScreen::screens(mtm);
    (0..screens.count())
        .filter_map(|i| screens.get(i))
        .map(|s| ScreenGeom {
            frame: s.frame().into(),
            visible: s.visibleFrame().into(),
        })
        .collect()
}

/// Show the pill on the screen under the mouse cursor.
///
/// Must be called on the main thread (use [`AppHandle::run_on_main_thread`]).
/// Uses `orderFrontRegardless`, which shows the window without activating
/// DictateFlow or taking keyboard focus — focus must stay in the app the
/// user is dictating into, or the ⌘V paste would land in the wrong place.
pub fn show_at_cursor<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    let mtm =
        MainThreadMarker::new().ok_or_else(|| anyhow!("show_at_cursor called off main thread"))?;
    let window = app
        .get_webview_window(PILL_LABEL)
        .ok_or_else(|| anyhow!("pill window '{PILL_LABEL}' not found"))?;
    let ns_window = ns_window_ref(&window)?;

    configure_overlay(ns_window);

    let screens = connected_screens(mtm);
    // SAFETY: class method with no preconditions; returns global coords.
    let mouse = unsafe { NSEvent::mouseLocation() };
    match pill_origin(&screens, (mouse.x, mouse.y)) {
        Some((x, y)) => {
            debug!(x, y, screens = screens.len(), "positioning pill");
            // SAFETY: property setter on a live NSWindow, on the main thread.
            unsafe { ns_window.setFrameOrigin(NSPoint::new(x, y)) };
        }
        None => warn!("no screens reported; showing pill at its last position"),
    }

    // SAFETY: as above.
    unsafe { ns_window.orderFrontRegardless() };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(x: f64, y: f64, w: f64, h: f64, dock: f64, menubar: f64) -> ScreenGeom {
        ScreenGeom {
            frame: Rect { x, y, w, h },
            visible: Rect {
                x,
                y: y + dock,
                w,
                h: h - dock - menubar,
            },
        }
    }

    // MacBook Air 13" (1512×982 pt) with Dock at the bottom.
    fn laptop() -> ScreenGeom {
        screen(0.0, 0.0, 1512.0, 982.0, 70.0, 37.0)
    }

    #[test]
    fn single_screen_centers_above_dock() {
        let (x, y) = pill_origin(&[laptop()], (100.0, 100.0)).unwrap();
        assert_eq!(x, ((1512.0 - PILL_WIDTH) / 2.0_f64).round());
        assert_eq!(y, 70.0 + BOTTOM_MARGIN);
    }

    #[test]
    fn external_monitor_to_the_right_uses_its_origin() {
        // 2560×1440 external to the right; no Dock on it.
        let ext = screen(1512.0, 0.0, 2560.0, 1440.0, 0.0, 25.0);
        let (x, y) = pill_origin(&[laptop(), ext], (3000.0, 700.0)).unwrap();
        assert_eq!(x, 1512.0 + ((2560.0 - PILL_WIDTH) / 2.0_f64).round());
        assert_eq!(y, BOTTOM_MARGIN);
        // Regression: must land inside the external screen, not the laptop.
        assert!(ext.frame.contains(x, y) && ext.frame.contains(x + PILL_WIDTH, y + PILL_HEIGHT));
    }

    #[test]
    fn external_monitor_to_the_left_has_negative_x() {
        let ext = screen(-1920.0, 0.0, 1920.0, 1080.0, 0.0, 25.0);
        let (x, _) = pill_origin(&[laptop(), ext], (-500.0, 400.0)).unwrap();
        assert!(x < 0.0 && x > -1920.0);
    }

    #[test]
    fn external_monitor_above_laptop() {
        let ext = screen(-200.0, 982.0, 1920.0, 1080.0, 0.0, 25.0);
        let (_, y) = pill_origin(&[laptop(), ext], (500.0, 1500.0)).unwrap();
        assert_eq!(y, 982.0 + BOTTOM_MARGIN);
    }

    #[test]
    fn cursor_outside_all_screens_falls_back_to_primary() {
        let ext = screen(1512.0, 0.0, 2560.0, 1440.0, 0.0, 25.0);
        let got = pill_origin(&[laptop(), ext], (-9999.0, -9999.0));
        assert_eq!(got, pill_origin(&[laptop()], (10.0, 10.0)));
    }

    #[test]
    fn cursor_on_shared_edge_picks_a_screen() {
        let ext = screen(1512.0, 0.0, 2560.0, 1440.0, 0.0, 25.0);
        assert!(pill_origin(&[laptop(), ext], (1512.0, 500.0)).is_some());
    }

    #[test]
    fn no_screens_returns_none() {
        assert_eq!(pill_origin(&[], (0.0, 0.0)), None);
    }
}
