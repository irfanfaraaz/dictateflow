//! Global hotkey listener.
//!
//! Web analogue: a global hotkey is like an `addEventListener('keydown')` on
//! `window`, except "window" is the entire OS — the keypress fires regardless
//! of which app is focused. This is the load-bearing feature for hold-to-talk:
//! the user is in Slack, holds F5, and we catch it.
//!
//! On macOS this requires the *Input Monitoring* permission (and silently
//! fails until granted — the plugin doesn't surface a permission error).
//! See `permissions::check_input_monitoring`.

pub mod listener;
mod validate;

use thiserror::Error;

pub use validate::{same_binding, validate_binding, DEFAULT_BINDING};

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("failed to register global shortcut: {0}")]
    Register(String),
    #[error("invalid chord \"{0}\": {1}")]
    InvalidChord(String, String),
    #[error("forbidden chord \"{0}\": {1}")]
    ForbiddenChord(String, String),
}
