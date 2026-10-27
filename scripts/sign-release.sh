#!/usr/bin/env bash
# Sign SHA256SUMS.txt of a release folder with the update signing key and check the signature.
#   scripts/sign-release.sh <dir> [--key-file <pem>] [--pubkey <pem>]
# The private key (PKCS#8 PEM, Ed25519) comes from the environment variable UPDATE_SIGNING_KEY, or from --key-file.
# Writes <dir>/SHA256SUMS.txt.sig (the raw 64-byte signature over the exact bytes of SHA256SUMS.txt), then verifies it with
# the public key (default scripts/update-public-key.pem; --pubkey is for tests). Any failure exits non-zero.
# The key is copied to a temp file only for the one openssl call (umask 077) and removed on exit. Needs OpenSSL with Ed25519
# (`pkeyutl -rawin`); the LibreSSL in macOS /usr/bin does not have it.
set -euo pipefail
set +x
umask 077
here="$(cd "$(dirname "$0")" && pwd)"
dir="" key_file="" pubkey="$here/update-public-key.pem"
while [ $# -gt 0 ]; do
  case "$1" in
    --key-file) key_file="${2:?--key-file needs a path}"; shift 2 ;;
    --pubkey) pubkey="${2:?--pubkey needs a path}"; shift 2 ;;
    -*) echo "sign-release: unknown option $1" >&2; exit 2 ;;
    *) [ -z "$dir" ] || { echo "sign-release: one directory only" >&2; exit 2; }; dir="$1"; shift ;;
  esac
done
[ -n "$dir" ] || { echo "usage: sign-release.sh <dir> [--key-file <pem>] [--pubkey <pem>]" >&2; exit 2; }
[ -s "$dir/SHA256SUMS.txt" ] || { echo "sign-release: $dir/SHA256SUMS.txt is missing or empty" >&2; exit 1; }
openssl version
openssl pkeyutl -help 2>&1 | grep -- '-rawin' >/dev/null || { echo "sign-release: openssl pkeyutl -rawin is required (found: $(openssl version))" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'if [ -f "$tmp/key.pem" ]; then shred -u "$tmp/key.pem" 2>/dev/null || rm -f "$tmp/key.pem"; fi; rm -rf "$tmp"' EXIT
if [ -n "$key_file" ]; then
  [ -s "$key_file" ] || { echo "sign-release: key file $key_file is missing or empty" >&2; exit 1; }
  cp "$key_file" "$tmp/key.pem"
elif [ -n "${UPDATE_SIGNING_KEY:-}" ]; then
  printf '%s\n' "$UPDATE_SIGNING_KEY" > "$tmp/key.pem"
else
  echo "sign-release: no signing key: set UPDATE_SIGNING_KEY or pass --key-file" >&2; exit 1
fi

rm -f "$dir/SHA256SUMS.txt.sig"
openssl pkeyutl -sign -inkey "$tmp/key.pem" -rawin -in "$dir/SHA256SUMS.txt" -out "$dir/SHA256SUMS.txt.sig" \
  || { echo "sign-release: signing failed (is the key an Ed25519 PKCS#8 PEM?)" >&2; rm -f "$dir/SHA256SUMS.txt.sig"; exit 1; }
size="$(wc -c < "$dir/SHA256SUMS.txt.sig" | tr -d ' ')"
[ "$size" = 64 ] || { echo "sign-release: signature is $size bytes, expected 64" >&2; rm -f "$dir/SHA256SUMS.txt.sig"; exit 1; }
openssl pkeyutl -verify -pubin -inkey "$pubkey" -rawin -in "$dir/SHA256SUMS.txt" -sigfile "$dir/SHA256SUMS.txt.sig" \
  || { echo "sign-release: the signature does not verify with $pubkey (wrong key in UPDATE_SIGNING_KEY?)" >&2; rm -f "$dir/SHA256SUMS.txt.sig"; exit 1; }
echo "signed: $dir/SHA256SUMS.txt.sig (64 bytes), verified with $pubkey"
