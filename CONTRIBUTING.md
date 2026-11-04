# Contributing

Thanks for helping. DictateFlow is a small macOS app with one job: press a key, talk, and get text in the app you're using. The bar for a change is whether it keeps that loop fast and reliable.

## Set up

You need macOS on Apple Silicon, the Rust toolchain ([rustup](https://rustup.rs)), Node.js 18+ and [pnpm](https://pnpm.io).

```sh
git clone https://github.com/irfanfaraaz/dictateflow
cd dictateflow
pnpm install
pnpm tauri dev
```

You'll need an [ElevenLabs](https://elevenlabs.io) API key with Scribe v2 access to dictate, and the Microphone, Accessibility and Input Monitoring permissions described in the README. Dev builds are ad-hoc signed, so macOS may ask for Accessibility again after a rebuild.

## Before you open a pull request

CI runs these on macOS. Run them locally first:

```sh
pnpm exec tsc -b                # TypeScript
pnpm exec vitest run            # frontend tests
cd src-tauri
cargo fmt --all -- --check      # formatting
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Clippy must pass with zero warnings.

## How changes land

- Branch from `main` and open a pull request into `main`. Keep each pull request to one change.
- Write commit messages in the [Conventional Commits](https://www.conventionalcommits.org) style the history already uses, for example `fix(inject): restore clipboard after paste` or `feat(pill): countdown near the duration cap`.
- Add a line to the `Unreleased` section of [CHANGELOG.md](CHANGELOG.md) for anything a user would notice.
- Update the README if you change setup, permissions, settings or behaviour.

## Where things live

| Path | What |
|---|---|
| `src-tauri/src/audio/` | Microphone capture (cpal), resampling to 16 kHz mono, voice activity detection |
| `src-tauri/src/engine/` | Transcription engines: ElevenLabs Scribe v2 (WebSocket) and local Whisper |
| `src-tauri/src/hotkey/` | The global hotkey, hold and toggle modes |
| `src-tauri/src/inject/` | Pasting the transcript: clipboard plus a synthetic ⌘V |
| `src-tauri/src/keychain.rs` | API key storage in the macOS Keychain |
| `src/pages/pill/` | The floating dictation pill and its waveform |
| `src/pages/settings/` | The settings window |

## Reporting bugs and ideas

Use the [issue forms](https://github.com/irfanfaraaz/dictateflow/issues/new/choose). For security problems, follow [SECURITY.md](SECURITY.md) instead of opening a public issue.

By contributing, you agree that your contributions are licensed under the project's [AGPL-3.0 licence](LICENSE).
