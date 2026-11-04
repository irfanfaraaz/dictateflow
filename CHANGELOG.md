# Changelog

All notable changes to DictateFlow. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow [Semantic Versioning](https://semver.org).

## Unreleased

### Added

- Licensed under AGPL-3.0, with contributing guide, code of conduct, security policy, issue forms and a pull request template.
- Demo video and GIF in the README.

## [0.3.1] - 2026-11-03

### Changed

- Releases are signed with a stable self-signed certificate, so Accessibility and Keychain grants carry over between versions.

### Fixed

- The API key is read from the Keychain once per launch instead of on every hotkey press.
- The release workflow verifies the signing certificate with `codesign -dvv`.

## [0.3.0] - 2026-11-03

### Added

- The pill shows messages for errors and notices, a success tick after pasting, and a countdown near the engine's duration cap.
- macOS CI: typecheck, frontend tests, `cargo fmt`, `cargo clippy`, `cargo test` and an install-script check.

### Changed

- The dictation lifecycle is serialised, audio is resampled to 16 kHz mono, and the engines and hotkeys are hardened.
- Settings, downloads, `open_path`, the content security policy and app launch are hardened.
- The curl installer and the release pipeline are hardened.

### Fixed

- The pill now appears on external monitors.
- Pasting restores the clipboard safely, checks that focus hasn't changed, sends a clean ⌘V and checks the microphone really produces sound.
- The installer works with macOS's bash 3.2.

## [0.2.0] - 2026-11-03

### Added

- One-line `curl` installer that avoids the Gatekeeper prompt.
- macOS release workflow that builds the `.dmg`, with manual runs from a tag.
- App icon, and a "check for updates" button in Settings → About.

### Changed

- Redesigned pill with a smoother lifecycle and animation.

### Fixed

- The waveform no longer freezes during dictation.

## [0.1.0] - 2026-05-07

### Added

- First release: global hotkey, floating pill, ElevenLabs Scribe v2 realtime transcription, optional local Whisper engine, input device picker, API key in the macOS Keychain, and paste into the focused app.

[0.3.1]: https://github.com/irfanfaraaz/dictateflow/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/irfanfaraaz/dictateflow/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/irfanfaraaz/dictateflow/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/irfanfaraaz/dictateflow/releases/tag/v0.1.0
