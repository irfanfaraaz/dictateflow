#!/usr/bin/env bash
# One-time setup: give DictateFlow releases a stable code signature.
#
#   curl -fsSL https://raw.githubusercontent.com/irfanfaraaz/dictateflow/main/scripts/setup-signing.sh | bash
#
# Why: macOS ties Accessibility permission and Keychain access to an app's
# code signature. Without a certificate every release gets a new throwaway
# (ad-hoc) signature, so each update looks like a different app and you have
# to re-grant Accessibility and re-approve the Keychain. Signing every build
# with the same certificate makes macOS recognise updates as the same app.
#
# What it does, all on this Mac:
#   1. creates a free self-signed code-signing certificate (valid 10 years);
#   2. saves it to ~/.dictateflow-signing (keep this: a new certificate means
#      one more re-grant);
#   3. uploads it to the repo's GitHub Actions secrets with `gh` if you have
#      it, otherwise copies each secret to your clipboard and tells you where
#      to paste it.
# The private key never leaves your Mac except as an encrypted GitHub secret.

set -euo pipefail

main() {
  local REPO="irfanfaraaz/dictateflow"
  local NAME="DictateFlow Self-Signed"
  local DIR="$HOME/.dictateflow-signing"

  info() { printf '\033[1;31m▍\033[0m %s\n' "$*"; }
  fail() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
  pause() { printf '%s' "$*"; read -r _ </dev/tty; }

  [[ "$(uname -s)" == "Darwin" ]] || fail "Run this on your Mac."

  # macOS's bundled /usr/bin/openssl (LibreSSL) writes .p12 files that the
  # macOS keychain can import; Homebrew's OpenSSL 3 needs -legacy for that.
  local ossl=/usr/bin/openssl legacy=()
  [[ -x "$ossl" ]] || ossl="$(command -v openssl)" || fail "openssl not found."
  "$ossl" version | grep -q '^OpenSSL 3' && legacy=(-legacy)

  if [[ -f "$DIR/signing.p12" ]]; then
    info "Reusing the certificate already in $DIR."
  else
    info "Creating a self-signed code-signing certificate…"
    mkdir -p "$DIR" && chmod 700 "$DIR"
    cat >"$DIR/cert.cnf" <<CNF
[req]
distinguished_name = dn
prompt = no
x509_extensions = ext
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF
    "$ossl" req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$DIR/cert.cnf" \
      -keyout "$DIR/key.pem" -out "$DIR/cert.pem" 2>/dev/null
    "$ossl" rand -hex 24 >"$DIR/password.txt"
    "$ossl" pkcs12 -export "${legacy[@]+"${legacy[@]}"}" -name "$NAME" \
      -inkey "$DIR/key.pem" -in "$DIR/cert.pem" \
      -passout "file:$DIR/password.txt" -out "$DIR/signing.p12"
    rm -f "$DIR/key.pem" "$DIR/cert.cnf"
    chmod 600 "$DIR"/*
  fi

  local p12_b64 password
  p12_b64="$(base64 <"$DIR/signing.p12" | tr -d '\n')"
  password="$(tr -d '\n' <"$DIR/password.txt")"

  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    info "Uploading secrets to $REPO with gh…"
    printf '%s' "$p12_b64" | gh secret set MACOS_SIGNING_P12 --repo "$REPO"
    printf '%s' "$password" | gh secret set MACOS_SIGNING_P12_PASSWORD --repo "$REPO"
  else
    local url="https://github.com/$REPO/settings/secrets/actions/new"
    info "Add two secrets at: $url"
    open "$url" 2>/dev/null || true
    printf '%s' "$p12_b64" | pbcopy
    pause "  1) Name: MACOS_SIGNING_P12  (value is on your clipboard). Paste, save, then press Enter… "
    printf '%s' "$password" | pbcopy
    pause "  2) Name: MACOS_SIGNING_P12_PASSWORD  (value is on your clipboard). Paste, save, then press Enter… "
    printf '' | pbcopy
  fi

  info "Done. Every release from now on is signed with \"$NAME\"."
  info "After the first signed update, re-grant Accessibility once and click \"Always Allow\""
  info "on the Keychain prompt. Later updates keep both."
}

main "$@"
