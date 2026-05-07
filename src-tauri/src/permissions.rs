//! macOS permission probes via thin FFI shims.
//!
//! TCC (Transparency, Consent and Control) is the macOS daemon that
//! mediates microphone, accessibility, input-monitoring grants. There is
//! no first-class Rust crate that wraps all three, so we link directly
//! against `ApplicationServices` (Accessibility), `IOKit` (Input
//! Monitoring), and `AVFoundation` (Microphone) and call a single C
//! function from each.
//!
//! # Why not request permission here?
//!
//! Requesting a permission triggers a TCC prompt and (for mic / IM) is
//! a one-shot — once denied, the only way back is the System Settings
//! deep-link the user clicks. We deliberately only *check* status here
//! and let `open_system_settings` deep-link the rest. Phase 9 may add
//! an explicit request flow on first dictation.
//!
//! # Web analogue
//!
//! Think `navigator.permissions.query({name: "microphone"})`, except
//! every check costs an FFI call and there's no async event when the
//! state flips. The frontend polls every 2 s while the Permissions tab
//! is mounted.

#![allow(unsafe_code, dead_code)] // FFI shims; unsafe is load-bearing

use serde::{Deserialize, Serialize};
use tracing::warn;

/// Tri-state mirror of macOS's TCC authorization status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    NotRequested,
}

/// Three permissions wispertype needs, each with its own System Settings pane.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    /// Microphone — required for `cpal` to deliver audio frames.
    Microphone,
    /// Accessibility — required for `CGEvent` synthetic ⌘V.
    Accessibility,
    /// Input Monitoring — required for global hotkeys on F-row / function keys.
    InputMonitoring,
}

// ----------------------------------------------------------------------
// Accessibility — `AXIsProcessTrusted` from ApplicationServices.
// ----------------------------------------------------------------------

// `AXIsProcessTrusted` returns Apple's `Boolean`, which is a `typedef
// unsigned char` (any value 0..=255 is legal). Rust's `bool` requires
// exactly 0 or 1, so binding the FFI return as `bool` is undefined
// behaviour even though the symbol always returns 0/1 in practice.
// We bind as `u8` and compare to zero at the call site.
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

/// Real probe: `AXIsProcessTrusted` is `true` once the user has added the
/// wispertype binary to System Settings → Privacy & Security →
/// Accessibility. AX only reports trusted vs not-trusted — it never
/// distinguishes "denied" from "never asked" — so we collapse not-trusted
/// into `NotRequested` to surface a single "open settings" CTA.
pub fn check_accessibility() -> PermissionState {
    // SAFETY: Pure C call, no pointers exchanged, no shared mutable state.
    let trusted = unsafe { AXIsProcessTrusted() } != 0;
    if trusted {
        PermissionState::Granted
    } else {
        PermissionState::NotRequested
    }
}

// ----------------------------------------------------------------------
// Input Monitoring — `IOHIDCheckAccess` from IOKit.
// ----------------------------------------------------------------------

/// `kIOHIDRequestTypeListenEvent = 1` — the access type for "I want to
/// observe HID events globally," which is the same scope the Input
/// Monitoring TCC pane controls. See `<IOKit/hid/IOHIDLib.h>`.
const KIO_HID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;

// Apple's `IOHIDAccessType` enum, decoded inside `check_input_monitoring`:
//   0 = kIOHIDAccessTypeGranted
//   1 = kIOHIDAccessTypeDenied
//   2 = kIOHIDAccessTypeUnknown
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDCheckAccess(request_type: u32) -> u32;
}

/// Real probe: returns the OS's view of whether this binary has Input
/// Monitoring grant. Available on macOS 10.15+; we don't gate the call
/// because the deployment target is macOS 11+ (Apple Silicon-only).
pub fn check_input_monitoring() -> PermissionState {
    // SAFETY: Pure C call with a primitive arg / return.
    let raw = unsafe { IOHIDCheckAccess(KIO_HID_REQUEST_TYPE_LISTEN_EVENT) };
    match raw {
        0 => PermissionState::Granted,
        1 => PermissionState::Denied,
        _ => PermissionState::NotRequested,
    }
}

// ----------------------------------------------------------------------
// Microphone — pragmatic probe via cpal device enumeration.
// ----------------------------------------------------------------------

/// Pragmatic probe: `AVCaptureDevice.authorizationStatusForMediaType:` is
/// the real call but pulling AVFoundation through objc2 + msg_send! adds
/// noticeable build complexity for a check we already perform implicitly
/// on first dictation. Instead we ask cpal whether a default input device
/// is reachable; if it is, the OS has at least authorized us to enumerate
/// (mic-denied apps see `Err(NoDevice)` from cpal). False negatives are
/// possible in unusual configurations (multiple audio backends, virtual
/// devices). Phase 9 polish revisits with the AV Foundation call.
pub fn check_microphone() -> PermissionState {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    match host.default_input_device() {
        Some(dev) => {
            // Querying `default_input_config` exercises one more layer of
            // permission — some macs return a device handle but fail on
            // config when TCC blocks the actual stream.
            match dev.default_input_config() {
                Ok(_) => PermissionState::Granted,
                Err(e) => {
                    warn!(
                        ?e,
                        "mic default_input_config failed; reporting NotRequested"
                    );
                    PermissionState::NotRequested
                }
            }
        }
        None => PermissionState::NotRequested,
    }
}

// ----------------------------------------------------------------------
// Open System Settings deep-link — NSWorkspace::openURL.
// ----------------------------------------------------------------------

/// Open the Privacy & Security pane that lets the user grant `kind`.
///
/// Uses the documented `x-apple.systempreferences:` URL scheme. The call
/// runs on whatever thread invokes it; AppKit tolerates `openURL:` from
/// arbitrary threads in practice (the receiver dispatches internally to
/// the main thread before the panel actually loads). If a future macOS
/// release tightens this we'll need to bounce through `run_on_main_thread`.
pub fn open_system_settings(kind: PermissionKind) {
    use objc2::rc::Retained;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    let url_str = match kind {
        PermissionKind::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        PermissionKind::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        PermissionKind::InputMonitoring => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
    };

    // SAFETY: All ObjC sends go through autoreleased Retained handles;
    // none of the inner pointers escape this function. The URL string is
    // an NSString backed by `&str` for its lifetime.
    unsafe {
        let ns_url_str = NSString::from_str(url_str);
        let url: Option<Retained<NSURL>> = NSURL::URLWithString(&ns_url_str);
        let Some(url) = url else {
            warn!(url_str, "failed to parse System Settings deep-link URL");
            return;
        };
        let workspace = NSWorkspace::sharedWorkspace();
        let opened = workspace.openURL(&url);
        if !opened {
            warn!(url_str, "NSWorkspace.openURL returned false");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_state_serializes_snake_case() {
        let v = serde_json::to_value(PermissionState::NotRequested).expect("ser");
        assert_eq!(v, serde_json::json!("not_requested"));
    }

    /// The accessibility / input-monitoring / microphone probes call OS
    /// FFI directly — we can't assert on the *result* (CI may grant or
    /// deny either), but we can confirm they don't panic under normal
    /// test runtime conditions.
    #[test]
    fn probes_do_not_panic() {
        let _ = check_accessibility();
        let _ = check_input_monitoring();
        let _ = check_microphone();
    }
}
