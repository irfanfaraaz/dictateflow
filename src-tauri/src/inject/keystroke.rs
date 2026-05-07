//! Synthesize a ⌘V keystroke via CoreGraphics.
//!
//! Posting synthetic events is gated behind the macOS **Accessibility**
//! permission. When the grant is missing, `CGEventPost` returns silently
//! (CG has no in-band failure signal for permission denial) — the user
//! sees no paste happen. Phase 1's `permissions::check_accessibility` is
//! how we surface that to the UI; this module assumes the grant is in
//! place and only reports CG-level allocation failures.

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use super::InjectError;

/// US-ANSI virtual keycode for the 'v' key.
///
/// We hard-code the keycode because ⌘V is interpreted by the *receiving*
/// app via its current keyboard layout — and that layout is what the user
/// already uses to paste manually. So 9 ("v" on US, but also the physical
/// key paste binds to on most layouts) is correct. If we ever ship a
/// non-US-default build we'll revisit via `UCKeyTranslate`.
const KEYCODE_V: CGKeyCode = 9;

/// Post a Cmd-down + V-down + V-up + Cmd-up sequence to the HID tap.
///
/// Uses `HIDSystemState` source so the event flows through the same path
/// a physical keypress would. We only emit the V key with the Command
/// flag set; macOS infers the modifier-down/up from the flag bitmask, so
/// no separate Cmd press/release is needed.
pub fn post_paste() -> Result<(), InjectError> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| InjectError::Cgevent("CGEventSourceCreate returned null".into()))?;

    let down = CGEvent::new_keyboard_event(source.clone(), KEYCODE_V, true).map_err(|()| {
        InjectError::Cgevent("CGEventCreateKeyboardEvent (down) returned null".into())
    })?;
    down.set_flags(CGEventFlags::CGEventFlagCommand);

    let up = CGEvent::new_keyboard_event(source, KEYCODE_V, false).map_err(|()| {
        InjectError::Cgevent("CGEventCreateKeyboardEvent (up) returned null".into())
    })?;
    up.set_flags(CGEventFlags::CGEventFlagCommand);

    // CGEventPost returns void; we have no inline way to detect that the
    // Accessibility permission is missing. The pre-flight permission check
    // (Phase 1) is the user-visible recovery path.
    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);

    Ok(())
}
