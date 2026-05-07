//! Optional audible feedback using macOS system sounds.
//!
//! No audio assets are shipped — we call `NSSound::soundNamed:` with the
//! built-in system sound names. This avoids app bundle bloat and works
//! offline. Sounds chosen for minimal intrusiveness:
//!
//! - **Start**: "Tink"  — a brief, soft chime. Clear "begin" signal.
//! - **Stop**:  "Pop"   — a short, neutral pop. Complements Tink.
//!
//! Both functions hop to the macOS main thread via `run_on_main_thread`
//! (matching the pattern used by `show_pill_async`) before touching AppKit.
//! NSSound playback is documented as thread-safe, but main-thread dispatch
//! is the safe insurance when running from an async context.

// SAFETY: All objc2 calls are gated behind `run_on_main_thread` which ensures
// we are on the macOS main thread before touching AppKit.
#![allow(unsafe_code)]

use objc2::rc::Retained;
use objc2_app_kit::NSSound;
use objc2_foundation::NSString;
use tauri::{AppHandle, Runtime};
use tracing::warn;

/// Play the system "Tink" sound to signal dictation start.
///
/// Call only when `settings.sounds.start_enabled` is true.
pub fn play_start<R: Runtime>(app: &AppHandle<R>) {
    play_named(app, "Tink");
}

/// Play the system "Pop" sound to signal dictation stop (clean inject).
///
/// Call only when `settings.sounds.stop_enabled` is true.
pub fn play_stop<R: Runtime>(app: &AppHandle<R>) {
    play_named(app, "Pop");
}

/// Resolve `NSSound::soundNamed:` on the main thread and call `play()`.
fn play_named<R: Runtime>(app: &AppHandle<R>, name: &'static str) {
    if let Err(e) = app.run_on_main_thread(move || {
        let ns_name = NSString::from_str(name);
        // SAFETY: soundNamed: is safe to call on the main thread with a valid
        // NSString. play() is safe on the returned NSSound instance.
        let sound: Option<Retained<NSSound>> = unsafe { NSSound::soundNamed(&ns_name) };
        match sound {
            Some(s) => {
                let _ = unsafe { s.play() };
            }
            None => {
                // System sound not found — this can happen in a headless
                // or sandboxed environment. Non-fatal; log and continue.
                warn!(%name, "NSSound::soundNamed returned nil — sound skipped");
            }
        }
    }) {
        warn!(?e, "run_on_main_thread for sound dispatch failed");
    }
}
