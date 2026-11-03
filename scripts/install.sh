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
# Options (environment variables):
#   DICTATEFLOW_VERSION=v0.2.0   install a specific release (default: latest)
#   GITHUB_TOKEN=...             authenticate GitHub API calls (avoids the
#                                60 requests/hour anonymous rate limit)
#
# Everything lives inside main(), called on the last line: if this script is
# truncated mid-download, bash runs nothing instead of a partial script.

set -euo pipefail

main() {
  local REPO="irfanfaraaz/dictateflow"
  local APP_NAME="DictateFlow.app"
  local BUNDLE_ID="com.dictateflow.app"
  local INSTALL_DIR="/Applications"

  info() { printf '\033[1;31m▍\033[0m %s\n' "$*"; }
  fail() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

  [[ "$(uname -s)" == "Darwin" ]] || fail "DictateFlow is macOS-only."
  # `uname -m` says x86_64 inside a Rosetta terminal on Apple Silicon; ask
  # the hardware instead.
  [[ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" == "1" ]] \
    || fail "DictateFlow currently ships for Apple Silicon (M1 or later) only."

  local api_url
  if [[ -n "${DICTATEFLOW_VERSION:-}" ]]; then
    api_url="https://api.github.com/repos/${REPO}/releases/tags/${DICTATEFLOW_VERSION}"
  else
    api_url="https://api.github.com/repos/${REPO}/releases/latest"
  fi
  local -a auth=()
  [[ -n "${GITHUB_TOKEN:-}" ]] && auth=(-H "Authorization: Bearer ${GITHUB_TOKEN}")

  info "Finding release…"
  local json
  json="$(curl -fsSL "${auth[@]+"${auth[@]}"}" -H "Accept: application/vnd.github+json" "$api_url")" \
    || fail "Couldn't query GitHub releases (${api_url}). Rate-limited? Try again later or set GITHUB_TOKEN. Wrong DICTATEFLOW_VERSION?"

  # Pull the .dmg's download URL and sha256 digest with a real JSON parser
  # (JavaScript for Automation ships with every macOS; no python/jq needed).
  local picked dmg_url digest
  picked="$(osascript -l JavaScript -e '
    function run(argv) {
      const rel = JSON.parse(argv[0]);
      const a = (rel.assets || []).find((x) => /\.dmg$/.test(x.name || ""));
      if (!a) return "";
      return a.browser_download_url + "\n" + String(a.digest || "").replace(/^sha256:/, "");
    }' "$json" 2>/dev/null || true)"
  dmg_url="$(printf '%s\n' "$picked" | sed -n 1p)"
  digest="$(printf '%s\n' "$picked" | sed -n 2p)"
  [[ "$dmg_url" == https://* ]] || fail "No .dmg found in this release."

  local tmp mount
  tmp="$(mktemp -d)"
  mount="$tmp/mnt"
  # shellcheck disable=SC2064  # expand $tmp/$mount now, on purpose
  trap "hdiutil detach '$mount' -force -quiet >/dev/null 2>&1 || true; rm -rf '$tmp'" EXIT

  info "Downloading $(basename "$dmg_url")…"
  curl -fL --retry 3 --retry-delay 2 --progress-bar -o "$tmp/DictateFlow.dmg" "$dmg_url" \
    || fail "Download failed."

  if [[ -n "$digest" ]]; then
    info "Verifying checksum…"
    printf '%s  %s\n' "$digest" "$tmp/DictateFlow.dmg" | shasum -a 256 -c - >/dev/null 2>&1 \
      || fail "Checksum mismatch — the download is corrupt or was tampered with. Nothing was installed."
  else
    info "Release has no published checksum; skipping verification."
  fi

  info "Mounting disk image…"
  mkdir -p "$mount"
  hdiutil attach "$tmp/DictateFlow.dmg" -nobrowse -readonly -quiet -mountpoint "$mount" \
    || fail "Couldn't open the disk image."
  [[ -d "$mount/$APP_NAME" ]] || fail "$APP_NAME not found inside the disk image."

  local sudo=""
  [[ -w "$INSTALL_DIR" ]] || sudo="sudo"

  # Copy next to the old app first; only swap once the copy succeeded, so a
  # failure (disk full, Ctrl-C) never leaves the user without an app.
  local staged="$INSTALL_DIR/.${APP_NAME}.installing"
  info "Copying to $INSTALL_DIR…"
  $sudo rm -rf "$staged"
  $sudo ditto "$mount/$APP_NAME" "$staged" || { $sudo rm -rf "$staged"; fail "Copy failed — your existing install is untouched."; }
  $sudo xattr -dr com.apple.quarantine "$staged" 2>/dev/null || true

  # Quit a running copy (by bundle id — works whatever the binary is named)
  # and wait for it to actually exit before swapping the bundle.
  local running_pat="/${APP_NAME}/Contents/MacOS/"
  if pgrep -f "$running_pat" >/dev/null 2>&1; then
    info "Quitting running DictateFlow…"
    osascript -e "tell application id \"${BUNDLE_ID}\" to quit" >/dev/null 2>&1 || true
    local i
    for i in $(seq 1 20); do
      pgrep -f "$running_pat" >/dev/null 2>&1 || break
      sleep 0.5
    done
    pkill -f "$running_pat" 2>/dev/null || true
    sleep 0.5
  fi

  $sudo rm -rf "$INSTALL_DIR/$APP_NAME"
  $sudo mv "$staged" "$INSTALL_DIR/$APP_NAME"

  info "Done. Launching DictateFlow…"
  open "$INSTALL_DIR/$APP_NAME"
  info "If this is an update: macOS may ask for Accessibility again (unsigned builds get a new signature)."
}

main "$@"
