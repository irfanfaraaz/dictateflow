//! Text injection into the focused application.
//!
//! Strategy: snapshot the whole pasteboard → write the transcript as a
//! *transient* item (clipboard managers skip it) → wait for the user to
//! release modifier keys → confirm the target app still has focus →
//! synthesize ⌘V via `CGEvent` → ~400 ms later restore the snapshot,
//! unless something else wrote to the pasteboard in the meantime.
//!
//! Why clipboard + ⌘V instead of inserting text "directly"? macOS has no
//! cross-app text-insertion API; the clipboard route is what every Wispr-
//! Flow-style app uses. Trade-off: requires the **Accessibility**
//! permission (synthetic CGEvent posting is gated behind it). When it is
//! missing we don't post at all: the transcript is left on the clipboard
//! and [`InjectError::AccessibilityDenied`] tells the UI what to do.
//!
//! When the paste cannot happen (no Accessibility, focus moved) the
//! transcript stays on the clipboard so the user can press ⌘V manually.

#![allow(unsafe_code)] // NSWorkspace FFI in `frontmost_app_pid`

pub mod keystroke;
pub mod pasteboard;

use std::time::Duration;

use thiserror::Error;
use tracing::{debug, warn};

use crate::permissions::{self, PermissionState};

// Called by the integrator from the main-thread pill-show path.
#[allow(unused_imports)]
pub use keystroke::refresh_keyboard_layout;

/// Errors emitted by the inject pipeline.
///
/// Display strings are short (the pill shows them); longer guidance lives
/// in [`InjectError::recovery_hint`].
#[derive(Debug, Error)]
pub enum InjectError {
    /// Pasteboard read/write failed.
    #[error("Clipboard error: {0}")]
    Clipboard(String),

    /// CoreGraphics keystroke synthesis failed (event allocation).
    #[error("Paste failed: {0}")]
    Cgevent(String),

    /// The process is not Accessibility-trusted; ⌘V was not posted. The
    /// transcript was left on the clipboard.
    #[error("Allow Accessibility to paste — text copied")]
    AccessibilityDenied,

    /// The app that was focused when dictation started is no longer
    /// frontmost; ⌘V was not posted. The transcript was left on the
    /// clipboard.
    #[error("Focus changed — text copied, press ⌘V")]
    FocusChanged,
}

impl InjectError {
    /// Stable machine-readable code for `ErrorPayload.code`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::AccessibilityDenied => "accessibility_denied",
            Self::FocusChanged => "focus_changed",
            Self::Clipboard(_) | Self::Cgevent(_) => "inject_failed",
        }
    }

    /// Longer, user-facing recovery guidance for `ErrorPayload.recovery_hint`.
    pub fn recovery_hint(&self) -> Option<&'static str> {
        match self {
            Self::AccessibilityDenied => Some(permissions::ACCESSIBILITY_RECOVERY_HINT),
            Self::FocusChanged => Some(
                "The app you were dictating into lost focus, so nothing was pasted. \
                 The text is on your clipboard — press ⌘V to paste it.",
            ),
            Self::Clipboard(_) | Self::Cgevent(_) => None,
        }
    }
}

/// Result of a successful [`paste`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasteOutcome {
    /// ⌘V was posted.
    #[allow(dead_code)] // part of the public contract; not every caller reads it
    pub pasted: bool,
    /// A restore of the prior clipboard was scheduled (it still skips
    /// itself if the user copies something before it runs).
    pub restored_clipboard: bool,
}

/// Pasteboard write-settle delay before posting ⌘V.
///
/// A freshly-written pasteboard can be missed by consumers that read
/// "too fast"; 50 ms is imperceptible (the pill is still up).
const PASTEBOARD_SETTLE: Duration = Duration::from_millis(50);

/// How long after ⌘V we wait before restoring the prior clipboard. The
/// target app reads the pasteboard while handling the keystroke; restoring
/// too early pastes the *old* contents.
const RESTORE_DELAY: Duration = Duration::from_millis(400);

/// Pid of the frontmost app (`NSWorkspace.frontmostApplication
/// .processIdentifier`). Callable from any thread. `None` if unknown.
pub fn frontmost_app_pid() -> Option<i32> {
    use objc2_app_kit::NSWorkspace;

    objc2::rc::autoreleasepool(|_| {
        // SAFETY: `sharedWorkspace` has no preconditions; NSWorkspace is
        // thread-safe. `frontmostApplication` may be nil (handled), and
        // `processIdentifier` is a plain getter on a live object.
        let pid = unsafe {
            let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
            app.processIdentifier()
        };
        // -1 means "no process" for NSRunningApplication.
        (pid > 0).then_some(pid)
    })
}

/// Decide whether it's safe to post ⌘V given the pid captured at the
/// start of dictation, the current frontmost pid, and our own pid.
///
/// With no target we can't verify anything and post as before. With a
/// target, the frontmost app must be that target and must not be
/// DictateFlow itself (our own windows never receive dictation).
pub fn focus_still_on_target(target: Option<i32>, frontmost: Option<i32>, own_pid: i32) -> bool {
    match target {
        None => true,
        Some(t) => frontmost == Some(t) && frontmost != Some(own_pid),
    }
}

fn own_pid() -> i32 {
    // Pids fit in i32 (pid_t) on macOS; fall back to an impossible value.
    i32::try_from(std::process::id()).unwrap_or(-1)
}

async fn blocking<T, F>(f: F) -> Result<T, InjectError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, InjectError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| InjectError::Cgevent(format!("blocking task join: {e}")))?
}

/// Paste `text` into the frontmost app via the clipboard + ⌘V.
///
/// `target_pid` is the frontmost app's pid captured when dictation began
/// (see [`frontmost_app_pid`]); if focus moved away we don't post and
/// return [`InjectError::FocusChanged`] with the text on the clipboard.
///
/// `text` must already be trimmed of empty/silence sentinels by the
/// caller — we paste exactly what we're given.
pub async fn paste(text: &str, target_pid: Option<i32>) -> Result<PasteOutcome, InjectError> {
    debug_assert!(
        !text.trim().is_empty(),
        "inject::paste called with empty text — caller must filter"
    );
    let text = text.to_owned();

    // No prompt in the hot path: prompting lives in the explicit
    // onboarding/settings request. Untrusted → leave the text on the
    // clipboard for a manual ⌘V and tell the UI.
    if permissions::check_accessibility() != PermissionState::Granted {
        warn!("accessibility not granted — leaving transcript on clipboard");
        blocking(move || pasteboard::write_plain(&text)).await?;
        return Err(InjectError::AccessibilityDenied);
    }

    let (snapshot, change_count) = blocking(move || {
        let snap = pasteboard::snapshot();
        let cc = pasteboard::write_transient(&text)?;
        Ok((snap, cc))
    })
    .await?;

    tokio::time::sleep(PASTEBOARD_SETTLE).await;

    // Modifier wait + focus check + post all happen on one blocking
    // thread so the focus check is as close to the keystroke as possible.
    let posted = blocking(move || {
        keystroke::wait_for_modifier_release();
        let frontmost = frontmost_app_pid();
        if !focus_still_on_target(target_pid, frontmost, own_pid()) {
            warn!(?target_pid, ?frontmost, "focus changed — not posting ⌘V");
            return Ok(false);
        }
        keystroke::post_paste()?;
        Ok(true)
    })
    .await?;

    if !posted {
        // Leave the transcript on the clipboard (no restore).
        return Err(InjectError::FocusChanged);
    }

    // Fire-and-forget restore; failures are logged inside the task.
    drop(tokio::spawn(async move {
        tokio::time::sleep(RESTORE_DELAY).await;
        match blocking(move || pasteboard::restore_if_unchanged(snapshot, change_count)).await {
            Ok(true) => debug!("prior clipboard restored"),
            Ok(false) => debug!("clipboard changed since paste; skipping restore"),
            Err(e) => warn!(?e, "clipboard restore failed"),
        }
    }));

    Ok(PasteOutcome {
        pasted: true,
        restored_clipboard: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_check_without_target_always_posts() {
        assert!(focus_still_on_target(None, None, 100));
        assert!(focus_still_on_target(None, Some(5), 100));
    }

    #[test]
    fn focus_check_requires_target_frontmost() {
        assert!(focus_still_on_target(Some(5), Some(5), 100));
        assert!(!focus_still_on_target(Some(5), Some(6), 100));
        assert!(!focus_still_on_target(Some(5), None, 100));
    }

    #[test]
    fn focus_check_rejects_own_process() {
        assert!(!focus_still_on_target(Some(100), Some(100), 100));
    }

    #[test]
    fn error_messages_are_short() {
        for e in [InjectError::AccessibilityDenied, InjectError::FocusChanged] {
            assert!(e.to_string().chars().count() <= 60, "{e}");
            assert!(e.recovery_hint().is_some());
        }
        assert_eq!(
            InjectError::AccessibilityDenied.code(),
            "accessibility_denied"
        );
        assert_eq!(InjectError::FocusChanged.code(), "focus_changed");
    }
}
