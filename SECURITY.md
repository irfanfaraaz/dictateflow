# Security

## Reporting a vulnerability

Please report security issues privately through [GitHub's advisory form](https://github.com/irfanfaraaz/dictateflow/security/advisories/new), not in a public issue.

Include what you found, how to reproduce it, your macOS version and the DictateFlow version (Settings → About). You'll get a reply within a week.

## Scope

In scope: the app and its install script. Examples: anything that exposes your ElevenLabs API key outside the macOS Keychain, sends audio or text anywhere other than the transcription engine you chose, pastes into an app other than the one in focus, keeps recording after you release the hotkey, or lets another process drive the app through its permissions. Problems in `scripts/install.sh` and in how releases are signed and downloaded are in scope too.

Out of scope: how ElevenLabs handles audio once it reaches their API (see their policies), and issues that need an attacker who already controls your user account.

## Supported versions

Only the [latest release](https://github.com/irfanfaraaz/dictateflow/releases/latest) gets fixes. Run the one-line installer again to update.
