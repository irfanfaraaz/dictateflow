#!/usr/bin/env bash
# Download ggml-small.bin into the DictateFlow app data dir.
#
# Prefer Settings → Models in the app (it verifies, resumes and registers the
# model). This script is a developer convenience; after running it, open
# Settings → Models once so the app picks the model up.
#
# Usage: ./scripts/download-model.sh
#
# - Refuses to overwrite an existing file.
# - Verifies the SHA-256 after download.
# - Does NOT touch the repo (the model must never be committed).

set -euo pipefail

MODEL_ID="ggml-small.bin"
URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${MODEL_ID}"
EXPECTED_SHA="1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"

DEST_DIR="${HOME}/Library/Application Support/com.dictateflow.app/models"
DEST_PATH="${DEST_DIR}/${MODEL_ID}"

mkdir -p "${DEST_DIR}"
# Never leave a half-written .partial behind on failure / Ctrl-C.
trap 'rm -f "${DEST_PATH}.partial"' ERR INT TERM

if [[ -f "${DEST_PATH}" ]]; then
  echo "model already exists at: ${DEST_PATH}"
  echo "delete it first if you want to re-download."
  exit 0
fi

echo "downloading ${MODEL_ID} (~466 MB)…"
curl -L --fail -o "${DEST_PATH}.partial" "${URL}"

echo "verifying SHA-256…"
ACTUAL_SHA=$(shasum -a 256 "${DEST_PATH}.partial" | awk '{print $1}')
if [[ "${ACTUAL_SHA}" != "${EXPECTED_SHA}" ]]; then
  echo "SHA-256 mismatch!"
  echo "  expected: ${EXPECTED_SHA}"
  echo "  actual:   ${ACTUAL_SHA}"
  rm -f "${DEST_PATH}.partial"
  exit 1
fi

mv "${DEST_PATH}.partial" "${DEST_PATH}"
echo "installed: ${DEST_PATH}"
