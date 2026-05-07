//! Pasteboard save / set / restore.
//!
//! `arboard` opens a fresh NSPasteboard handle per `Clipboard::new()` —
//! cheap on macOS, but importantly NOT `Send` across `.await` in some
//! builds. We always create-use-drop within a single sync block.
//!
//! v1 limitation: only text round-trips faithfully. If the prior clipboard
//! held an image, file reference, RTF, or anything else non-textual, we
//! capture `text: None` and the [`restore`] becomes a no-op rather than
//! corrupting the prior contents with an empty string. Documented in
//! JOURNEY §09; users with image-clipboards are warned that dictateflow
//! will leave their pasteboard unchanged after dictation.

use arboard::Clipboard;

use super::InjectError;

/// Snapshot of clipboard contents we know how to restore.
///
/// Future expansion: an `image: Option<Vec<u8>>` field could capture
/// arboard's `ImageData` for raster images. Not done in v1 because:
/// (a) the encode/decode roundtrip is lossy unless we keep the raw NSImage,
/// (b) the byte cost of holding a 24-bit screenshot in memory for 250 ms
/// is non-trivial, and (c) the spec narrows v1 to text dictation.
#[derive(Debug, Default, Clone)]
pub struct ClipboardSnapshot {
    pub(crate) text: Option<String>,
}

/// Read the current pasteboard text into a snapshot.
///
/// A pasteboard with no text content (image, file, empty, …) yields
/// `text: None` rather than an error — that's a normal state on macOS,
/// not a failure mode.
pub fn save() -> Result<ClipboardSnapshot, InjectError> {
    let mut cb = Clipboard::new()?;
    let text = match cb.get_text() {
        Ok(s) => Some(s),
        Err(arboard::Error::ContentNotAvailable) => None,
        Err(e) => return Err(InjectError::Clipboard(e)),
    };
    Ok(ClipboardSnapshot { text })
}

/// Overwrite the pasteboard with `s`. The caller is responsible for the
/// 50 ms write-settle delay before consuming the value (see
/// [`super::PASTEBOARD_SETTLE`]).
pub fn set_text(s: &str) -> Result<(), InjectError> {
    let mut cb = Clipboard::new()?;
    cb.set_text(s.to_owned())?;
    Ok(())
}

/// Restore a previously-saved snapshot. No-op when the snapshot held no
/// restorable text content — see [`ClipboardSnapshot`] for why we don't
/// try to round-trip non-text payloads in v1.
pub fn restore(snapshot: ClipboardSnapshot) -> Result<(), InjectError> {
    if let Some(text) = snapshot.text {
        let mut cb = Clipboard::new()?;
        cb.set_text(text)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "uses real clipboard"]
    fn round_trip_text() {
        // This test mutates the host pasteboard — opt-in via `cargo test --
        // --ignored`. Skipped on headless CI.
        let original = "dictateflow-clipboard-test-marker";
        {
            let mut cb = Clipboard::new().expect("open clipboard");
            cb.set_text(original.to_owned()).expect("seed clipboard");
        }

        let snap = save().expect("save snapshot");
        set_text("replacement value").expect("set replacement");
        restore(snap).expect("restore");

        let mut cb = Clipboard::new().expect("re-open clipboard");
        let after = cb.get_text().expect("read clipboard after restore");
        assert_eq!(after, original);
    }
}
