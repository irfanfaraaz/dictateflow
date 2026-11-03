//! Synthesize a clean ⌘V keystroke via CoreGraphics.
//!
//! Three things make a naive "post V with the Command flag" unreliable:
//!
//! 1. **Held modifiers.** A `HIDSystemState` event source merges the
//!    *physical* modifier state into every event it creates. The Toggle
//!    hotkey stops on key-down, so Cmd+Shift may still be held and the
//!    app receives ⌘⇧V (often "paste and match style" or nothing). We
//!    wait (bounded) for all modifiers to be released, then post from a
//!    `Private` source whose events carry only the flags we set.
//! 2. **Implicit Command.** We post an explicit Cmd-down / V-down / V-up /
//!    Cmd-up sequence instead of relying on the flag bitmask alone, so
//!    apps that track modifier key events see a real chord.
//! 3. **Keyboard layout.** Keycode 9 is "v" on QWERTY but "." on Dvorak.
//!    [`refresh_keyboard_layout`] resolves the keycode that types "v" on
//!    the current layout (via `UCKeyTranslate`) and caches it.
//!
//! Posting is gated behind the Accessibility permission; `CGEventPost`
//! has no in-band failure signal, so the caller checks trust first.

#![allow(unsafe_code)] // CoreGraphics / Carbon FFI; every block carries a SAFETY note

use std::ffi::c_void;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

use core_foundation::base::{CFRelease, CFTypeRef};
use core_foundation::data::{CFDataGetBytePtr, CFDataRef};
use core_foundation::string::CFStringRef;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use tracing::{debug, warn};

use super::InjectError;

/// US-ANSI virtual keycode for "v" — fallback when layout lookup fails.
pub const DEFAULT_KEYCODE_V: CGKeyCode = 9;
/// `kVK_Command` (left Command key).
const KEYCODE_COMMAND: CGKeyCode = 55;

/// Keycode that types "v" on the current layout; written by
/// [`refresh_keyboard_layout`] on the main thread, read on paste.
static KEYCODE_V: AtomicU16 = AtomicU16::new(DEFAULT_KEYCODE_V);

/// Upper bound on waiting for the user to release modifier keys.
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(500);
const MODIFIER_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// `kCGEventSourceStateHIDSystemState`.
const HID_SYSTEM_STATE: i32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// `CGEventSourceFlagsState(CGEventSourceStateID) -> CGEventFlags`.
    fn CGEventSourceFlagsState(state_id: i32) -> u64;
}

/// Modifiers that change what ⌘V means to the receiving app.
fn chord_modifier_mask() -> u64 {
    (CGEventFlags::CGEventFlagCommand
        | CGEventFlags::CGEventFlagShift
        | CGEventFlags::CGEventFlagAlternate
        | CGEventFlags::CGEventFlagControl)
        .bits()
}

/// True when none of Cmd / Shift / Option / Control are held in `flags`.
pub fn modifiers_released(flags: u64) -> bool {
    flags & chord_modifier_mask() == 0
}

/// Block (≤ [`MODIFIER_RELEASE_TIMEOUT`]) until the physical modifier keys
/// are up. Returns whether they were released in time. Call from a
/// blocking thread.
pub fn wait_for_modifier_release() -> bool {
    let deadline = Instant::now() + MODIFIER_RELEASE_TIMEOUT;
    loop {
        // SAFETY: pure C call with a valid state-id constant; no pointers.
        let flags = unsafe { CGEventSourceFlagsState(HID_SYSTEM_STATE) };
        if modifiers_released(flags) {
            return true;
        }
        if Instant::now() >= deadline {
            warn!(
                flags,
                "modifiers still held after timeout; posting ⌘V anyway"
            );
            return false;
        }
        std::thread::sleep(MODIFIER_POLL_INTERVAL);
    }
}

fn key_event(
    source: &CGEventSource,
    keycode: CGKeyCode,
    down: bool,
    flags: CGEventFlags,
) -> Result<CGEvent, InjectError> {
    let ev = CGEvent::new_keyboard_event(source.clone(), keycode, down).map_err(|()| {
        InjectError::Cgevent(format!(
            "CGEventCreateKeyboardEvent(keycode {keycode}, down {down}) returned null"
        ))
    })?;
    ev.set_flags(flags);
    Ok(ev)
}

/// Post Cmd-down, V-down, V-up, Cmd-up from a private event source, so
/// only the Command flag reaches the target app. Call after
/// [`wait_for_modifier_release`].
pub fn post_paste() -> Result<(), InjectError> {
    let source = CGEventSource::new(CGEventSourceStateID::Private)
        .map_err(|()| InjectError::Cgevent("CGEventSourceCreate returned null".into()))?;
    let v = KEYCODE_V.load(Ordering::Relaxed);
    let cmd = CGEventFlags::CGEventFlagCommand;

    // Build everything before posting so an allocation failure never
    // leaves a dangling Cmd-down in the event stream.
    let events = [
        key_event(&source, KEYCODE_COMMAND, true, cmd)?,
        key_event(&source, v, true, cmd)?,
        key_event(&source, v, false, cmd)?,
        key_event(&source, KEYCODE_COMMAND, false, CGEventFlags::empty())?,
    ];
    for ev in &events {
        ev.post(CGEventTapLocation::HID);
    }
    debug!(keycode = v, "posted ⌘V");
    Ok(())
}

// ----------------------------------------------------------------------
// Keyboard layout → keycode for "v" (Carbon Text Input Sources).
// ----------------------------------------------------------------------

/// `kUCKeyActionDisplay`.
const UC_KEY_ACTION_DISPLAY: u16 = 3;
/// `1 << kUCKeyTranslateNoDeadKeysBit`.
const UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK: u32 = 1;

#[link(name = "Carbon", kind = "framework")]
#[allow(non_upper_case_globals)] // Apple symbol names
extern "C" {
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
    fn TISGetInputSourceProperty(source: CFTypeRef, key: CFStringRef) -> *const c_void;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(
        key_layout_ptr: *const c_void,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        key_translate_options: u32,
        dead_key_state: *mut u32,
        max_string_length: usize,
        actual_string_length: *mut usize,
        unicode_string: *mut u16,
    ) -> i32;
}

/// First keycode in `0..128` whose unmodified output is exactly `target`.
pub fn find_keycode(translate: impl Fn(u16) -> Option<String>, target: &str) -> Option<u16> {
    (0u16..128).find(|&k| translate(k).as_deref() == Some(target))
}

/// Translate `keycode` with no modifiers through the `uchr` layout data.
///
/// # Safety
/// `layout` must point to valid `UCKeyboardLayout` data that outlives the call.
unsafe fn translate_keycode(layout: *const c_void, kbd_type: u32, keycode: u16) -> Option<String> {
    let mut dead_key_state: u32 = 0;
    let mut len: usize = 0;
    let mut buf = [0u16; 4];
    // SAFETY: `layout` is valid per the caller contract; the out-pointers
    // reference live stack locals and `buf.len()` bounds the write.
    let status = unsafe {
        UCKeyTranslate(
            layout,
            keycode,
            UC_KEY_ACTION_DISPLAY,
            0,
            kbd_type,
            UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK,
            &mut dead_key_state,
            buf.len(),
            &mut len,
            buf.as_mut_ptr(),
        )
    };
    if status != 0 || len == 0 || len > buf.len() {
        return None;
    }
    String::from_utf16(&buf[..len]).ok()
}

/// Look up the keycode for "v" on the current keyboard layout.
fn lookup_keycode_v() -> Option<u16> {
    // SAFETY: returns a +1 retained TISInputSourceRef or null; we check
    // null and CFRelease it on every path below.
    let source = unsafe { TISCopyCurrentKeyboardLayoutInputSource() };
    if source.is_null() {
        return None;
    }
    // SAFETY: `source` is a live input source; the property key is a
    // Carbon-provided constant. The returned CFData is borrowed (Get rule)
    // and stays valid while `source` is retained.
    let data = unsafe { TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) };
    let result = if data.is_null() {
        None
    } else {
        // SAFETY: `data` is a non-null CFDataRef owned by `source`.
        let layout = unsafe { CFDataGetBytePtr(data as CFDataRef) } as *const c_void;
        if layout.is_null() {
            None
        } else {
            // SAFETY: pure C getter, no arguments.
            let kbd_type = u32::from(unsafe { LMGetKbdType() });
            find_keycode(
                // SAFETY: `layout` points into CFData kept alive by `source`,
                // which is released only after this scan finishes.
                |k| unsafe { translate_keycode(layout, kbd_type, k) },
                "v",
            )
        }
    };
    // SAFETY: balances the +1 from TISCopyCurrentKeyboardLayoutInputSource.
    unsafe { CFRelease(source) };
    result
}

/// Resolve and cache the virtual keycode that types "v" on the current
/// keyboard layout. MUST be called on the main thread (TIS asserts off
/// main on macOS 14+); off-main calls are ignored with a warning. Any
/// lookup failure keeps the previously cached value (initially 9).
#[allow(dead_code)] // called from the main-thread pill-show path
pub fn refresh_keyboard_layout() {
    if !objc2_foundation::is_main_thread() {
        warn!("refresh_keyboard_layout called off the main thread; ignoring");
        return;
    }
    match lookup_keycode_v() {
        Some(k) => {
            let prev = KEYCODE_V.swap(k, Ordering::Relaxed);
            if prev != k {
                debug!(keycode = k, "keyboard layout: \"v\" keycode updated");
            }
        }
        None => debug!("keyboard layout lookup failed; keeping cached \"v\" keycode"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_released_only_when_no_chord_modifier_held() {
        assert!(modifiers_released(0));
        // Caps Lock / Fn / numeric-pad bits don't block the paste.
        assert!(modifiers_released(
            (CGEventFlags::CGEventFlagAlphaShift | CGEventFlags::CGEventFlagSecondaryFn).bits()
        ));
        for f in [
            CGEventFlags::CGEventFlagCommand,
            CGEventFlags::CGEventFlagShift,
            CGEventFlags::CGEventFlagAlternate,
            CGEventFlags::CGEventFlagControl,
        ] {
            assert!(!modifiers_released(f.bits()), "{f:?} should block");
        }
        assert!(!modifiers_released(
            (CGEventFlags::CGEventFlagCommand | CGEventFlags::CGEventFlagShift).bits()
        ));
    }

    #[test]
    fn find_keycode_picks_first_match() {
        // Dvorak: "v" lives on keycode 47.
        let dvorak = |k: u16| match k {
            9 => Some(".".to_string()),
            47 => Some("v".to_string()),
            _ => None,
        };
        assert_eq!(find_keycode(dvorak, "v"), Some(47));
        assert_eq!(find_keycode(|_| None, "v"), None);
        assert_eq!(
            find_keycode(|k| (k >= 3).then(|| "v".to_string()), "v"),
            Some(3)
        );
    }
}
