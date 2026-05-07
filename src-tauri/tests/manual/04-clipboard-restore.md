# 04 — clipboard restore

verify the drop-guard restores the clipboard after paste.

## steps

1. copy the string `test123` to the clipboard (Cmd+C in any text editor).
2. focus a plain text field (e.g. a Notes window).
3. dictate "hello world" (hold hotkey ~1.5 s, release).
4. observe the text `hello world` appears in the focused field.
5. immediately open any app that can show clipboard contents (e.g. paste a new text field with Cmd+V).
6. the pasted value should be `test123` — the original clipboard content.

## expected

- `hello world` pastes into the focused field.
- clipboard returns to `test123` within ~250 ms after paste.
- no stale intermediate clipboard state is observable by the user.

## notes

- the restore is synchronous in the drop-guard, so it happens before the Rust function returns. the 250 ms margin is UI polling lag, not a real delay.
- if clipboard does not restore, check that the ClipboardGuard drop impl runs even on the inject-error path.
