# DictateFlow

Voice dictation for macOS — speak into any app, get text. Built with Rust + Tauri 2.

Press a hotkey, talk, release. Your speech is transcribed via ElevenLabs Scribe v2 and pasted into whatever app has focus. A minimal floating waveform bubble shows at the bottom of your screen while recording.

## Download

Grab the latest `.dmg` from [**Releases**](https://github.com/irfanfaraaz/dictateflow/releases).

> **macOS Gatekeeper warning** — DictateFlow is not yet code-signed, so macOS will block it on first launch. To open it:
>
> 1. Open the `.dmg` and drag DictateFlow to Applications
> 2. Open Terminal and run:
>    ```bash
>    xattr -cr /Applications/DictateFlow.app
>    ```
> 3. Now open DictateFlow normally
>
> You only need to do this once. The `xattr` command removes the quarantine flag macOS adds to downloaded apps.

### Requirements

- **macOS** on Apple Silicon (M1+)
- **ElevenLabs API key** ([elevenlabs.io](https://elevenlabs.io) — Scribe v2 access)

### Build from source

Requires Rust toolchain (`rustup`), Node.js 18+, and pnpm.

```bash
git clone https://github.com/irfanfaraaz/dictateflow.git
cd dictateflow
pnpm install
pnpm tauri build
```

The `.app` lands in `src-tauri/target/release/bundle/macos/DictateFlow.app` and the `.dmg` in `src-tauri/target/release/bundle/dmg/`.

## Setup

1. **Launch the app** — the Settings window opens.
2. **Add your ElevenLabs API key** in Settings → Engine. The key is stored in macOS Keychain.
3. **Pick your microphone** in Settings → Engine → Input Device (defaults to system default).
4. **Set your hotkey** in Settings → Hotkey (default: F5). Toggle or hold-to-talk modes.
5. **Grant permissions** when prompted:

| Permission | Why | How |
|---|---|---|
| **Microphone** | Audio capture during dictation | macOS prompts automatically on first use |
| **Accessibility** | Synthetic ⌘V to paste transcript | System Settings → Privacy → Accessibility → toggle DictateFlow ON |
| **Input Monitoring** | Global hotkey on F-row keys | System Settings → Privacy → Input Monitoring → toggle DictateFlow ON |

After granting Accessibility, **quit and relaunch** the app (macOS caches the grant per-process).

## Usage

1. Focus any text input (editor, browser, chat, terminal)
2. Press your hotkey — the floating bubble appears at the bottom of your screen
3. Speak naturally
4. Release the hotkey (or press again in toggle mode)
5. Text is pasted into the focused app

## Stack

| Concern | Choice |
|---|---|
| Desktop framework | Tauri 2.x |
| Frontend | React + Tailwind + framer-motion |
| Backend | Rust, Tokio async runtime |
| Audio | cpal → 16kHz mono → ring buffer |
| Transcription | ElevenLabs Scribe v2 realtime (WebSocket) |
| Local engine | whisper.cpp via whisper-rs (Metal) — optional |
| Secrets | macOS Keychain |
| Text injection | Clipboard + CGEvent ⌘V |

## Development

```bash
# Dev mode with hot reload
pnpm tauri dev

# Clippy lint (must pass with zero warnings)
cd src-tauri && cargo clippy --all-targets --all-features -- -D warnings

# Run tests
cargo test

# TypeScript check
npx tsc --noEmit
```

Logs are written to `~/Library/Logs/dictateflow/dictateflow.log.*`.

## Notes

- **Bluetooth earbuds**: Work, but the app correctly handles the HFP profile switch delay. If your earbuds have poor mic quality, use Settings to pick your built-in Mac mic instead.
- **After rebuilding**: macOS TCC grants are tied to the binary hash. You'll need to re-grant Accessibility permission in System Settings after each rebuild (remove the old entry, relaunch, re-grant).
- **Local Whisper engine**: Available but requires downloading a GGML model in Settings → Models. ElevenLabs Scribe is the primary engine.

## License

Private — not yet licensed for distribution.
