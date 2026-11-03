//! Text injection into the focused application.
//!
//! Strategy: save current pasteboard → write transcript text → synthesize
//! ⌘V via `CGEvent` → schedule restore of the prior pasteboard ~250 ms
//! later (via the [`guard::ClipboardGuard`] Drop impl).
//!
//! Why clipboard + ⌘V instead of inserting text "directly"? macOS has no
//! cross-app text-insertion API; the clipboard route is what every Wispr-
//! Flow-style app uses. Trade-off: requires the **Accessibility**
//! permission (synthetic CGEvent posting is gated behind it). When the
//! permission is missing, posting silently no-ops and the user sees the
//! pill fade with nothing pasted; the Phase 1 `permissions::check_accessibility`
//! check is the recovery hint surface.
//!
//! Web analogue: simulating keystrokes from JS is forbidden in browsers
//! by design (security boundary). On macOS native, it's possible but
//! gated behind a per-app user grant.

#![allow(dead_code)] // wired in over the course of phase 3

pub mod clipboard;
pub mod guard;
pub mod keystroke;

use std::time::Duration;

use thiserror::Error;
use tracing::warn;

/// Errors emitted by the inject pipeline.
///
/// `arboard::Error` already carries good Display strings; we forward them
/// transparently. CGEvent failures stringify what we know — the macOS
/// API gives us very little signal beyond "creation returned null".
#[derive(Debug, Error)]
pub enum InjectError {
    #[error("clipboard error: {0}")]
    Clipboard(#[from] arboard::Error),

    /// CoreGraphics keystroke synthesis failed. Most common cause: the
    /// Accessibility permission has not been granted to DictateFlow.
    #[error("CGEvent error: {0}")]
    Cgevent(String),
}

/// Pasteboard write-settle delay.
///
/// macOS NSPasteboard writes are observable to other processes only after
/// they propagate through the per-process pasteboard cache. In practice a
/// freshly-set string can be missing if the consumer reads "too fast" —
/// 30–50 ms is the empirical floor. We pick 50 ms for headroom; it's
/// imperceptible to the user (the pill is still up) and avoids the
/// "first paste eats the prior clipboard" failure mode.
const PASTEBOARD_SETTLE: Duration = Duration::from_millis(50);

/// Save the current clipboard, write `text`, post ⌘V, and schedule the
/// clipboard restore for ~250 ms later (via the dropped guard).
///
/// `text` must already have been trimmed of empty/silence sentinels by
/// the caller — we paste exactly what we're given. Empty input is treated
/// as a programming error in debug builds and a quiet no-op in release.
pub async fn paste(text: &str) -> Result<(), InjectError> {
    debug_assert!(
        !text.trim().is_empty(),
        "inject::paste called with empty text — caller must filter"
    );

    if !crate::permissions::request_accessibility() {
        warn!("accessibility not granted — synthetic ⌘V will be silently dropped by macOS");
    }

    clipboard::set_text(text)?;

    // Let the pasteboard write propagate. Without this, fast consumers
    // (e.g. apps that re-read on focus) sometimes paste the prior contents.
    tokio::time::sleep(PASTEBOARD_SETTLE).await;

    // CGEventPost is sync C-FFI — keep it off the async executor by hopping
    // to a blocking thread. It also avoids any subtle Send issues with the
    // CGEvent handles across .await suspension points.
    tokio::task::spawn_blocking(keystroke::post_paste)
        .await
        .map_err(|e| InjectError::Cgevent(format!("join: {e}")))??;

    Ok(())
}
