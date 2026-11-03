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

/// Three permissions DictateFlow needs, each with its own System Settings pane.
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
/// DictateFlow binary to System Settings → Privacy & Security →
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

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> u8;
}

/// Check accessibility and show the system prompt if not yet trusted.
/// Returns true if already trusted, false if the prompt was shown.
pub fn request_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::true_value();
    let pairs: Vec<(CFString, CFBoolean)> = vec![(key, value)];
    let opts = CFDictionary::from_CFType_pairs(&pairs);
    // SAFETY: `opts` is a live CFDictionary for the duration of the call;
    // AX only reads it and does not retain beyond the call.
    (unsafe { AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef() as *const _) }) != 0
}

/// Recovery guidance when Accessibility is not trusted.
///
/// Unsigned / ad-hoc-signed builds get a new code signature on every
/// update, so System Settings can show the toggle ON while
/// `AXIsProcessTrusted` is false for the new binary. Removing and
/// re-adding the entry is the only fix.
pub const ACCESSIBILITY_RECOVERY_HINT: &str = "Accessibility permission is off for this build. \
In System Settings → Privacy & Security → Accessibility, remove DictateFlow (−), \
add it again (+), then relaunch.";

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
// Microphone — `AVCaptureDevice.authorizationStatusForMediaType:`.
// ----------------------------------------------------------------------

#[link(name = "AVFoundation", kind = "framework")]
#[allow(non_upper_case_globals)] // Apple symbol names
extern "C" {
    /// `AVMediaTypeAudio` (an `AVMediaType`, i.e. `NSString *`).
    static AVMediaTypeAudio: &'static objc2_foundation::NSString;
}

/// Map `AVAuthorizationStatus` to our tri-state.
///
/// 0 NotDetermined → `NotRequested` (the system will prompt the first
/// time cpal opens the input stream), 1 Restricted / 2 Denied → `Denied`,
/// 3 Authorized → `Granted`. Unknown future values are treated as
/// `NotRequested` so the UI keeps offering the settings CTA.
pub fn map_av_authorization_status(status: isize) -> PermissionState {
    match status {
        3 => PermissionState::Granted,
        1 | 2 => PermissionState::Denied,
        _ => PermissionState::NotRequested,
    }
}

/// Real TCC probe for the microphone.
///
/// Enumerating CoreAudio devices (the previous approach) succeeds even
/// when TCC denies capture, so it reported `Granted` for denied apps.
/// `AVCaptureDevice` asks TCC directly.
///
/// We don't request access programmatically here: that needs an ObjC
/// block (`requestAccessForMediaType:completionHandler:`) and `block2`
/// is not a direct dependency. Opening the input stream via cpal on the
/// first dictation triggers the same system prompt.
pub fn check_microphone() -> PermissionState {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;

    let Some(cls) = AnyClass::get("AVCaptureDevice") else {
        warn!("AVCaptureDevice class not found; reporting NotRequested");
        return PermissionState::NotRequested;
    };
    // SAFETY: `authorizationStatusForMediaType:` is a class method taking
    // an `AVMediaType` (NSString) and returning `AVAuthorizationStatus`
    // (NSInteger). `AVMediaTypeAudio` is a framework-provided constant
    // that lives for the whole process.
    let status: isize =
        unsafe { msg_send![cls, authorizationStatusForMediaType: AVMediaTypeAudio] };
    map_av_authorization_status(status)
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
    fn av_status_maps_to_permission_state() {
        assert_eq!(
            map_av_authorization_status(0),
            PermissionState::NotRequested
        );
        assert_eq!(map_av_authorization_status(1), PermissionState::Denied);
        assert_eq!(map_av_authorization_status(2), PermissionState::Denied);
        assert_eq!(map_av_authorization_status(3), PermissionState::Granted);
        assert_eq!(
            map_av_authorization_status(99),
            PermissionState::NotRequested
        );
    }

    #[test]
    fn accessibility_hint_has_reset_steps() {
        assert!(ACCESSIBILITY_RECOVERY_HINT
            .starts_with("Accessibility permission is off for this build. In System Settings"));
        assert!(ACCESSIBILITY_RECOVERY_HINT.ends_with("add it again (+), then relaunch."));
    }

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
