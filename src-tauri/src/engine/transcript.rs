//! Transcript post-processing shared by all engines.
//!
//! Whisper (and occasionally Scribe) emit non-speech annotations on silence
//! or background noise: `[BLANK_AUDIO]`, `[Music]`, `(silence)`, `♪`, …
//! Pasting those into the user's document is never what they want.

/// Clean an engine transcript for pasting.
///
/// - Every `[...]` group is removed (whisper only uses square brackets for
///   non-speech tags such as `[BLANK_AUDIO]`, `[Music]`, `[inaudible]`).
/// - `(...)` / `*...*` groups and music notes are removed only when the
///   transcript consists of *nothing but* such annotations — a real sentence
///   that happens to contain "(see above)" is kept verbatim.
/// - Whitespace is collapsed.
///
/// Returns `None` when nothing pasteable is left.
pub fn clean_transcript(text: &str) -> Option<String> {
    let without_brackets = strip_groups(text, '[', ']');
    let collapsed = collapse_ws(&without_brackets);
    if collapsed.is_empty() {
        return None;
    }

    // Only-annotations check: strip parenthesised / starred groups and music
    // notes; if nothing word-like is left, the whole thing was non-speech.
    let probe = strip_groups(&strip_groups(&collapsed, '(', ')'), '*', '*');
    let has_words = probe
        .chars()
        .any(|c| c.is_alphanumeric() && c != '♪' && c != '♫');
    if !has_words {
        return None;
    }
    Some(collapsed)
}

/// Remove every `open … close` group (non-nested; an unclosed `open` keeps
/// the rest of the text).
fn strip_groups(text: &str, open: char, close: char) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let after = &rest[start + open.len_utf8()..];
        match after.find(close) {
            Some(end) => {
                out.push_str(&rest[..start]);
                out.push(' ');
                rest = &after[end + close.len_utf8()..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

fn collapse_ws(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Removing a tag can leave "hello , world" — tidy the space before
    // common punctuation.
    joined
        .replace(" ,", ",")
        .replace(" .", ".")
        .replace(" !", "!")
        .replace(" ?", "?")
        .trim()
        .to_string()
}

/// Truncate `s` to at most `max_bytes` bytes on a char boundary (for logs).
pub fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let end = s
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= max_bytes)
        .last()
        .unwrap_or(0);
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_audio_only_is_dropped() {
        assert_eq!(clean_transcript("[BLANK_AUDIO]"), None);
        assert_eq!(clean_transcript("  [BLANK_AUDIO]  [BLANK_AUDIO] "), None);
    }

    #[test]
    fn non_speech_annotations_only_are_dropped() {
        assert_eq!(clean_transcript("(silence)"), None);
        assert_eq!(clean_transcript("[Music]"), None);
        assert_eq!(clean_transcript("(music) [Music] *coughs*"), None);
        assert_eq!(clean_transcript("♪ ♪"), None);
        assert_eq!(clean_transcript(""), None);
        assert_eq!(clean_transcript("   "), None);
        assert_eq!(clean_transcript("( wind blowing )."), None);
    }

    #[test]
    fn real_speech_is_kept() {
        assert_eq!(
            clean_transcript("Hello world."),
            Some("Hello world.".to_string())
        );
    }

    #[test]
    fn bracket_tags_are_stripped_from_speech() {
        assert_eq!(
            clean_transcript("[BLANK_AUDIO] Hello there [Music] friend."),
            Some("Hello there friend.".to_string())
        );
        assert_eq!(
            clean_transcript("Hello [inaudible], world"),
            Some("Hello, world".to_string())
        );
    }

    #[test]
    fn parentheses_inside_speech_are_kept() {
        assert_eq!(
            clean_transcript("Use the second option (see above)."),
            Some("Use the second option (see above).".to_string())
        );
    }

    #[test]
    fn unclosed_bracket_keeps_text() {
        assert_eq!(
            clean_transcript("array[0 is fine"),
            Some("array[0 is fine".to_string())
        );
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "héllo wörld";
        // 'é' spans bytes 1..3 — cutting at 2 must not split it.
        assert_eq!(truncate_on_char_boundary(s, 2), "h");
        assert_eq!(truncate_on_char_boundary(s, 3), "hé");
        assert_eq!(truncate_on_char_boundary(s, 100), s);
        assert_eq!(truncate_on_char_boundary("日本語", 4), "日");
        assert_eq!(truncate_on_char_boundary("", 4), "");
    }
}
