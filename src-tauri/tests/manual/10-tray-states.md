# 10 — tray icon states

verify the tray icon reflects app state within ~100 ms.

## steps

1. launch the app. observe the tray icon in the menu bar — it should show the idle state.
2. start dictation (hold hotkey). within ~100 ms the tray icon should switch to the listening state (different icon / badge / label depending on Phase 9 implementation).
3. hold for 2 s, then release. within ~100 ms the icon returns to idle.
4. trigger an error (scenario from QA 09). observe: tray icon may remain idle or briefly show an error state. either is acceptable as long as it resets after the chip dismisses.

## expected

- idle → listening transition: ≤ 100 ms from hotkey press.
- listening → idle transition: ≤ 100 ms from hotkey release or dictation:injected event.
- no stuck tray state after an error.

## notes

- tray state is driven by the same Rust event pipeline as the pill. if the pill updates correctly (QA 01–09), tray should follow.
- if tray icon assets are placeholder PNGs (Phase 9 deliverable), verify they at least swap on state change — final art comes in Phase 10 craft pass.
