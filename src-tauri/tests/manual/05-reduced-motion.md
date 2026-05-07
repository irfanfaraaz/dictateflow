# 05 — reduced motion

verify cross-fades collapse to snap transitions when Reduce Motion is enabled.

## setup

1. System Settings → Accessibility → Display → Reduce Motion → ON.

## steps

1. open settings window. switch between tabs (engine → hotkey → models).
2. the tab content should switch with no visible fade — an instant swap.
3. trigger dictation (hold hotkey ~1 s). the pill should appear immediately without a fade-in.
4. release the hotkey. the pill should disappear immediately without a fade-out.
5. trigger an error (revoke mic permission, then attempt dictation). the error chip height expansion should be instant, not animated.

## expected

- zero visible animation on all transitions.
- functionality is identical to the normal path (pill appears, waveform draws, text pastes).

## cleanup

1. System Settings → Accessibility → Display → Reduce Motion → OFF.
