# 01 — first-run permissions

fresh install; verify all three TCC grants succeed and the settings tab reflects them.

## setup

1. build a debug or release bundle. do not launch the app yet.
2. open System Settings → Privacy & Security and confirm wispertype has no existing entries under Microphone, Accessibility, or Input Monitoring.

## steps

1. launch wispertype (tray icon appears).
2. open settings → permissions tab. all three rows should read `not requested`.
3. press the global hotkey (default `F5`) to trigger dictation. macOS shows a microphone permission dialog.
4. click Allow. the microphone row in the permissions tab should change to `granted` within 2 s (one poll tick).
5. attempt dictation again. macOS shows an accessibility permission dialog.
6. open System Settings → Accessibility and enable wispertype. return to app.
7. accessibility row should now read `granted`.
8. if input monitoring is not yet granted, macOS prompts on the first global-shortcut registration (may have already occurred at step 3). grant it.
9. input monitoring row should read `granted`.

## expected

- all three rows show `granted` without app restart.
- no crash, no error chip in the pill.
- dictation captures audio and pastes text.
