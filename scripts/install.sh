#!/usr/bin/env bash
# DictateFlow installer.
#
#   curl -fsSL https://raw.githubusercontent.com/irfanfaraaz/dictateflow/main/scripts/install.sh | bash
#
# Why curl: browsers tag downloads with the `com.apple.quarantine` xattr,
# which makes Gatekeeper block unsigned apps. curl does not set that flag,
# so an app installed this way opens without the "unidentified developer"
# prompt. We also strip the flag explicitly, just in case.
#
# Optional: DICTATEFLOW_VERSION=v0.1.0 to pin a release (default: latest).

set -euo pipefail

REPO="irfanfaraaz/dictateflow"
APP_NAME="DictateFlow.app"
INSTALL_DIR="/Applications"

info() { printf '\033[1;31m▍\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

[[ "$(uname -s)" == "Darwin" ]] || fail "DictateFlow is macOS-only."
[[ "$(uname -m)" == "arm64" ]] || fail "DictateFlow currently ships for Apple Silicon (arm64) only."

if [[ -n "${DICTATEFLOW_VERSION:-}" ]]; then
  API_URL="https://api.github.com/repos/${REPO}/releases/tags/${DICTATEFLOW_VERSION}"
else
  API_URL="https://api.github.com/repos/${REPO}/releases/latest"
fi

info "Finding release…"
DMG_URL="$(curl -fsSL "$API_URL" \
  | grep -o '"browser_download_url": *"[^"]*\.dmg"' \
  | head -n 1 \
  | sed -E 's/.*"(https[^"]+)"/\1/')"
[[ -n "$DMG_URL" ]] || fail "No .dmg asset found in release ($API_URL)."

TMP="$(mktemp -d)"
MOUNT="$TMP/mnt"
cleanup() {
  hdiutil detach "$MOUNT" -quiet >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

info "Downloading $(basename "$DMG_URL")…"
curl -fL --progress-bar -o "$TMP/DictateFlow.dmg" "$DMG_URL"

info "Mounting disk image…"
mkdir -p "$MOUNT"
hdiutil attach "$TMP/DictateFlow.dmg" -nobrowse -readonly -quiet -mountpoint "$MOUNT"
[[ -d "$MOUNT/$APP_NAME" ]] || fail "$APP_NAME not found inside the disk image."

if pgrep -xq DictateFlow; then
  info "Quitting running DictateFlow…"
  osascript -e 'quit app "DictateFlow"' >/dev/null 2>&1 || pkill -x DictateFlow || true
  sleep 1
fi

SUDO=""
[[ -w "$INSTALL_DIR" ]] || SUDO="sudo"

info "Installing to $INSTALL_DIR…"
$SUDO rm -rf "$INSTALL_DIR/$APP_NAME"
$SUDO ditto "$MOUNT/$APP_NAME" "$INSTALL_DIR/$APP_NAME"
$SUDO xattr -dr com.apple.quarantine "$INSTALL_DIR/$APP_NAME" 2>/dev/null || true

info "Done. Launching DictateFlow…"
open "$INSTALL_DIR/$APP_NAME"
