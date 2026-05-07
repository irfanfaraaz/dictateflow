# 02 — engine swap mid-session

verify toggling from local to scribe (or back) takes effect on the next dictation without a restart.

## setup

- local engine: at least one model installed (settings → models).
- scribe engine: a valid ElevenLabs API key set (settings → engine → scribe card).

## steps

1. set engine to local. dictate a short phrase. confirm it pastes correctly.
2. open settings → engine. click the scribe v2 card.
3. do not restart the app. dictate a short phrase.
4. observe pill behaves normally; text pastes.
5. open settings → engine. click the local card.
6. dictate a short phrase. confirm local engine is used (latency is higher for scribe; local should be slightly faster but less accurate).

## expected

- each dictation uses the engine selected at start time (not the previous one).
- no error chip, no app restart required.
- switching from scribe to local when API key is absent falls back without a panic; error chip shows "set api key to use scribe v2".
