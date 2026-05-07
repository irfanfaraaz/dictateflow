# 09 — error states

verify the error chip appears with an actionable message in three failure modes, then auto-dismisses.

## scenario a — mic permission revoked

1. System Settings → Privacy & Security → Microphone → revoke wispertype.
2. attempt dictation (hold hotkey).
3. observe: pill appears with error chip. message should be imperative (e.g. `grant microphone access in system settings`).
4. error chip auto-dismisses after 3 s. pill hides.
5. restore mic permission (System Settings → Microphone → re-enable wispertype).

## scenario b — scribe with no API key

1. set engine to scribe. clear the API key if one is set (settings → engine → scribe card → [clear]).
2. attempt dictation.
3. observe: error chip with message like `set api key to use scribe v2`.
4. auto-dismisses after 3 s.

## scenario c — scribe with network disconnected

1. set engine to scribe. ensure a valid API key is set.
2. turn off Wi-Fi (or disconnect ethernet).
3. attempt dictation.
4. observe: error chip with a connection-related message (e.g. `connection failed` or `websocket error`).
5. auto-dismisses after 3 s.
6. restore network.

## expected for all scenarios

- error chip is red, single line, truncated if long.
- chip height is ~80 px (animated from 56 px, or instant if Reduce Motion is on).
- auto-dismisses after 3 s; pill hides.
- no crash.
