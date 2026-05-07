# 07 — engine cap and waveform warning

verify the 80% urgency signal and the hard cap auto-stop.

## setup

- local engine selected.
- cap is 60 s by default (settings → hotkey → cap row).

## steps

1. start dictation (hold or toggle hotkey). speak continuously or stay quiet — audio capture runs regardless.
2. watch the duration counter in the pill: `00:00 / 60`.
3. at ~48 s (80% of 60 s) the counter text turns bold and brighter red; the waveform bars switch to the urgency color.
4. continue. at 60 s the Rust cap timer fires `audio_stop`. the pill transitions to "pasting…" then hides.
5. confirm any audio captured before cap is transcribed and pasted.

## expected

- urgency signal appears between 48 s and 50 s.
- pill auto-hides at or near 60 s without user input.
- no error chip.
- text pastes (may be empty if you were silent; that's covered in QA 08).

## notes

- scribe cap is 300 s by default; adjust settings if you want to test that path without waiting 5 min. lower to 10 s for a faster cycle.
