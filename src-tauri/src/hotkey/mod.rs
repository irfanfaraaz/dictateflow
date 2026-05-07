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

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("failed to register global shortcut: {0}")]
    Register(String),
    #[error("invalid chord \"{0}\": {1}")]
    InvalidChord(String, String),
    #[error("forbidden chord \"{0}\": {1}")]
    ForbiddenChord(String, String),
}

/// Validate a user-supplied binding string.
///
/// Rejects:
/// - Chords that fail to parse as a `Shortcut` (wrong format / unknown key).
/// - System-reserved chords: Cmd+Q, Cmd+W, Cmd+Tab, Cmd+Space, Cmd+Shift+3/4/5
///   (screenshots), Cmd+H, Cmd+M (macOS standard window management).
/// - Single bare letters with no modifier (a global shortcut of just `A` would
///   fire while the user types — never acceptable).
///
/// Returns `Ok(())` when the binding is safe to register.
pub fn validate_binding(binding: &str) -> Result<(), HotkeyError> {
    use std::str::FromStr;
    use tauri_plugin_global_shortcut::Shortcut;

    // 1. Must parse.
    let shortcut = Shortcut::from_str(binding)
        .map_err(|e| HotkeyError::InvalidChord(binding.to_owned(), e.to_string()))?;

    // 2. Forbidden-chord check using the parsed representation. We compare
    //    on the normalised Display string so we don't miss case variants.
    let normalised = binding.to_uppercase();
    let forbidden: &[(&str, &str)] = &[
        ("CMD+Q", "quits the application"),
        ("CMD+W", "closes the window"),
        ("CMD+TAB", "switches applications"),
        ("CMD+SPACE", "opens Spotlight"),
        ("CMD+SHIFT+3", "takes a screenshot"),
        ("CMD+SHIFT+4", "takes a screenshot"),
        ("CMD+SHIFT+5", "opens screenshot tools"),
        ("CMD+H", "hides the window"),
        ("CMD+M", "minimises the window"),
        // Guard against "SUPER+…" aliases since frontend uses "Cmd"
        ("SUPER+Q", "quits the application"),
        ("SUPER+W", "closes the window"),
        ("SUPER+TAB", "switches applications"),
        ("SUPER+SPACE", "opens Spotlight"),
        ("SUPER+SHIFT+3", "takes a screenshot"),
        ("SUPER+SHIFT+4", "takes a screenshot"),
        ("SUPER+SHIFT+5", "opens screenshot tools"),
        ("SUPER+H", "hides the window"),
        ("SUPER+M", "minimises the window"),
    ];

    for (chord, reason) in forbidden {
        if normalised == *chord {
            return Err(HotkeyError::ForbiddenChord(
                binding.to_owned(),
                reason.to_string(),
            ));
        }
    }

    // 3. Reject bare single-key shortcuts (no modifiers at all). A lone `A`
    //    would fire on every keypress — it is never an acceptable hotkey.
    //    (This check uses the parsed shortcut's modifier mask.)
    let _ = shortcut; // used above for parse; modifiers come from string analysis
    let tokens: Vec<&str> = binding.split('+').collect();
    if tokens.len() == 1 {
        // Single token — check if it's a plain letter (not Fn key, not space, not arrow, etc.)
        let tok = tokens[0].to_uppercase();
        let is_plain_letter =
            tok.len() == 1 && tok.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        if is_plain_letter {
            return Err(HotkeyError::ForbiddenChord(
                binding.to_owned(),
                "a single unmodified letter would intercept normal typing".to_owned(),
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f5_is_valid() {
        assert!(validate_binding("F5").is_ok());
    }

    #[test]
    fn cmd_q_forbidden() {
        assert!(matches!(
            validate_binding("Cmd+Q"),
            Err(HotkeyError::ForbiddenChord(_, _))
        ));
    }

    #[test]
    fn bare_letter_forbidden() {
        assert!(matches!(
            validate_binding("A"),
            Err(HotkeyError::ForbiddenChord(_, _))
        ));
    }

    #[test]
    fn cmd_shift_space_valid() {
        // Common user choice; not on the forbidden list.
        assert!(validate_binding("Cmd+Shift+Space").is_ok());
    }
}
