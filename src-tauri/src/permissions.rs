//! macOS permission stubs.
//!
//! The real implementations land in Phase 4 (the Permissions tab in the
//! settings window needs them). For Phase 1 we expose the API surface so the
//! settings tab + dictation error chip code can compile in later phases —
//! every `check_*` returns `NotRequested` until we wire AVFoundation /
//! ApplicationServices.
//!
//! Web analogue: think of these as `navigator.permissions.query({ name })`,
//! except the browser sandbox doesn't exist — we have to talk to TCC, the
//! Transparency, Consent and Control daemon.

#![allow(dead_code)] // surface API; real impl in Phase 4

use serde::Serialize;

/// Tri-state mirror of macOS's TCC authorization status.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    NotRequested,
}

/// Three permissions wispertype needs, each with its own System Settings pane.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    /// Microphone — required for `cpal` to deliver audio frames.
    Microphone,
    /// Accessibility — required for `CGEvent` synthetic ⌘V.
    Accessibility,
    /// Input Monitoring — required for global hotkeys on F-row / function keys.
    InputMonitoring,
}

/// Phase 4 will replace this with `AVAudioApplication.recordPermission`.
pub fn check_microphone() -> PermissionState {
    PermissionState::NotRequested
}

/// Phase 4 will replace this with `AXIsProcessTrusted()` from
/// ApplicationServices.framework.
pub fn check_accessibility() -> PermissionState {
    PermissionState::NotRequested
}

/// Phase 4 will replace this with `IOHIDCheckAccess(kIOHIDRequestTypeListenEvent)`.
pub fn check_input_monitoring() -> PermissionState {
    PermissionState::NotRequested
}

/// Open the Privacy & Security pane that lets the user grant `kind`. Phase 4
/// will replace this stub with `NSWorkspace::openURL` against the
/// `x-apple.systempreferences:` deep link for the right pane.
pub fn open_system_settings(_kind: PermissionKind) {
    // no-op for Phase 1
}
