# 03 — cancel on hotkey restart

verify that re-pressing the hotkey during an active dictation cancels the in-flight session cleanly.

## steps (hold mode)

1. set hotkey mode to hold.
2. hold the hotkey for ~2 s. pill is visible, waveform animating.
3. without releasing, briefly release and immediately hold again (quick tap then re-hold).
4. observe: pill resets (timer goes back to 00:00), prior audio is discarded, no paste occurs.
5. release after another 2 s. new transcript pastes normally.

## steps (toggle mode)

1. set hotkey mode to toggle.
2. press hotkey once (starts dictation). wait 2 s.
3. press hotkey again to stop — text should paste.
4. immediately press hotkey again (starts new session). wait 1 s. press again to stop.
5. only the second session's text should paste; there should be no double-paste.

## expected

- no double-paste from the cancelled session.
- no error chip, no panic.
- pill hides cleanly after paste.
- clipboard is restored to its pre-dictation contents (see QA script 04).
