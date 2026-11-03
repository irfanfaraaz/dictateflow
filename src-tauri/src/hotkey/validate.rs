//! Hotkey binding validation.
//!
//! Bindings are compared on the *parsed* `(modifiers, key)` pair, never on the
//! raw string, so reordering (`Shift+Cmd+3`), aliases (`Command+Q`,
//! `Super+Q`) and case variants can't sneak a reserved chord past the check.

use std::str::FromStr;

use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

use super::HotkeyError;

/// Binding used when the stored one is missing or invalid.
pub const DEFAULT_BINDING: &str = "F5";

/// Validate a user-supplied binding string and return the parsed shortcut.
///
/// Rejects:
/// - Strings that fail to parse as a `Shortcut` (wrong format / unknown key,
///   including unsupported aliases such as `Meta+Q`).
/// - System-reserved chords: ⌘Q, ⌘W, ⌘Tab, ⌘Space, ⌘⇧3/4/5 (screenshots),
///   ⌘H, ⌘M, ⌥⌘Esc (Force Quit), ⌃Space (input source switch).
/// - Anything without a non-Shift modifier unless the key is F1–F20. A bare
///   `A`, `Space`, `Enter`, `Backspace`, digit, or `Shift+A` would swallow
///   normal typing system-wide while DictateFlow runs.
pub fn validate_binding(binding: &str) -> Result<Shortcut, HotkeyError> {
    let shortcut = Shortcut::from_str(binding.trim())
        .map_err(|e| HotkeyError::InvalidChord(binding.to_owned(), e.to_string()))?;

    let mods = shortcut.mods;
    let key = shortcut.key;

    if let Some(reason) = reserved_reason(mods, key) {
        return Err(HotkeyError::ForbiddenChord(
            binding.to_owned(),
            reason.to_owned(),
        ));
    }

    let has_real_modifier = mods.intersects(Modifiers::SUPER | Modifiers::CONTROL | Modifiers::ALT);
    if !has_real_modifier && !is_function_key(key) {
        return Err(HotkeyError::ForbiddenChord(
            binding.to_owned(),
            "without ⌘, ⌃ or ⌥ only F1–F20 are allowed — this key would intercept normal typing"
                .to_owned(),
        ));
    }

    Ok(shortcut)
}

/// Normalised identity of a binding, for "is this the same shortcut?"
/// comparisons (`"shift+cmd+space"` == `"Cmd+Shift+Space"`).
pub fn same_binding(a: &Shortcut, b: &Shortcut) -> bool {
    a.mods == b.mods && a.key == b.key
}

fn reserved_reason(mods: Modifiers, key: Code) -> Option<&'static str> {
    let cmd = Modifiers::SUPER;
    let cmd_shift = Modifiers::SUPER | Modifiers::SHIFT;
    let cmd_alt = Modifiers::SUPER | Modifiers::ALT;
    let reserved: &[(Modifiers, Code, &str)] = &[
        (cmd, Code::KeyQ, "quits the application"),
        (cmd, Code::KeyW, "closes the window"),
        (cmd, Code::Tab, "switches applications"),
        (cmd, Code::Space, "opens Spotlight"),
        (cmd, Code::KeyH, "hides the window"),
        (cmd, Code::KeyM, "minimises the window"),
        (cmd_shift, Code::Digit3, "takes a screenshot"),
        (cmd_shift, Code::Digit4, "takes a screenshot"),
        (cmd_shift, Code::Digit5, "opens screenshot tools"),
        (cmd_alt, Code::Escape, "opens Force Quit"),
        (Modifiers::CONTROL, Code::Space, "switches the input source"),
    ];
    reserved
        .iter()
        .find(|(m, k, _)| *m == mods && *k == key)
        .map(|(_, _, reason)| *reason)
}

fn is_function_key(key: Code) -> bool {
    matches!(
        key,
        Code::F1
            | Code::F2
            | Code::F3
            | Code::F4
            | Code::F5
            | Code::F6
            | Code::F7
            | Code::F8
            | Code::F9
            | Code::F10
            | Code::F11
            | Code::F12
            | Code::F13
            | Code::F14
            | Code::F15
            | Code::F16
            | Code::F17
            | Code::F18
            | Code::F19
            | Code::F20
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forbidden(b: &str) -> bool {
        matches!(validate_binding(b), Err(HotkeyError::ForbiddenChord(_, _)))
    }

    #[test]
    fn function_keys_are_valid() {
        assert!(validate_binding("F5").is_ok());
        assert!(validate_binding("f13").is_ok());
        assert!(validate_binding("F20").is_ok());
        assert!(validate_binding("Shift+F6").is_ok());
    }

    #[test]
    fn modified_chords_are_valid() {
        assert!(validate_binding("Cmd+Shift+Space").is_ok());
        assert!(validate_binding("Ctrl+Alt+D").is_ok());
        assert!(validate_binding("Option+Space").is_ok());
    }

    #[test]
    fn reserved_chords_are_forbidden_in_any_spelling() {
        for b in [
            "Cmd+Q",
            "cmd+q",
            "Command+Q",
            "Super+Q",
            "CMD+W",
            "Cmd+Tab",
            "Cmd+Space",
            "Cmd+Shift+3",
            "Shift+Cmd+3",
            "shift+super+4",
            "Command+Shift+5",
            "Cmd+H",
            "Cmd+M",
            "Cmd+Alt+Escape",
            "Ctrl+Space",
        ] {
            assert!(forbidden(b), "{b} should be forbidden");
        }
    }

    #[test]
    fn unknown_alias_is_rejected() {
        assert!(validate_binding("Meta+Q").is_err());
        assert!(validate_binding("Hyper+Q").is_err());
        assert!(validate_binding("").is_err());
    }

    #[test]
    fn typing_keys_without_modifier_are_forbidden() {
        for b in [
            "A",
            "Space",
            "Enter",
            "Backspace",
            "5",
            "Shift+A",
            "Shift+Space",
            "Tab",
            "F21",
        ] {
            assert!(validate_binding(b).is_err(), "{b} should be rejected");
        }
        assert!(forbidden("A"));
        assert!(forbidden("Shift+A"));
    }

    #[test]
    fn same_binding_ignores_spelling() {
        let a = validate_binding("Cmd+Shift+Space").expect("valid");
        let b = validate_binding("shift+command+SPACE").expect("valid");
        let c = validate_binding("Cmd+Alt+Space").expect("valid");
        assert!(same_binding(&a, &b));
        assert!(!same_binding(&a, &c));
    }
}
