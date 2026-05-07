# 08 — empty transcript (silence)

verify the app handles total silence gracefully — no paste, no crash, clean dismiss.

## steps

1. focus a text field.
2. start dictation. do not speak. wait 2–3 s.
3. stop dictation (release hold key, or press toggle key again).
4. observe:
   - pill shows "pasting…" briefly, then hides.
   - no text is inserted into the focused field.
   - clipboard is unchanged (see QA 04 for clipboard-restore verification).

## expected

- pill hides cleanly.
- no error chip.
- no paste, no empty string, no stray newline in the text field.
- no crash in the Rust side (check logs at ~/Library/Logs/wispertype/ if in doubt).

## notes

- whisper may emit an empty string or a hallucinated filler like `[BLANK_AUDIO]` for silence. the inject path should strip empty/whitespace-only transcripts and not call CGEvent paste. verify the text field is untouched.
