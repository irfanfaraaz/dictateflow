# Bundled resources

Files placed in this directory are picked up by Tauri's resource bundler (see
the `bundle.resources` glob in `src-tauri/tauri.conf.json`) and shipped next
to the executable inside the `.app` bundle. At runtime they are reachable via
`tauri::path::PathResolver::resolve(name, BaseDirectory::Resource)`.

## `ggml-small.bin` (Whisper model — required, NOT in git)

Phase 2 of Wispertype runs a single bundled GGML model: `ggml-small.bin`
(~466 MB). This file is **deliberately excluded from version control** —
`.gitignore` blocks `*.bin` at the repo root.

### Developer setup

The first time you build the app, download the model into one of two places:

1. **Recommended (no rebuild needed):** drop it into your app data dir so the
   path resolver finds it before falling back to the bundled resource:

   ```bash
   mkdir -p "$HOME/Library/Application Support/com.wispertype.app/models"
   curl -L -o "$HOME/Library/Application Support/com.wispertype.app/models/ggml-small.bin" \
     https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
   ```

2. **Alternative (ships in `.app` bundle):** drop it directly here at
   `src-tauri/resources/ggml-small.bin`. `pnpm tauri build` will then bundle
   it into the `.app`. Do NOT `git add` the file.

A helper script lives at `scripts/download-model.sh` that does option (1) for
you. It verifies the SHA-256 and refuses to overwrite an existing file.

### Checksum

`ggml-small.bin` SHA-256:
`1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b`

### Why two locations?

- **App data dir** is writable at runtime — Phase 5 lands a UI downloader
  that drops models there. Useful during development; no rebuild required.
- **Bundled resource** is read-only and ships with the app. Useful for
  shipping a "first-run-ready" build to end-users (Phase 2 only; Phase 5
  removes the bundled copy in favour of the in-app downloader).
