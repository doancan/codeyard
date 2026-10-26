#!/usr/bin/env bash
# Notarize an already signed DMG and portable binary folder. Does not rebuild or re-sign.
# NOTARY_PROFILE=... [NOTARY_KEYCHAIN=...] scripts/notarize-macos.sh <dmg> <portable-dir> [app]
set +x
set -euo pipefail
: "${NOTARY_PROFILE:?set NOTARY_PROFILE (notarytool keychain profile)}"
[ "$#" -ge 2 ] && [ "$#" -le 3 ] || { echo 'usage: notarize-macos.sh <signed.dmg> <portable-dir> [app]' >&2; exit 2; }
dmg=$1
portable=$2
app=${3:-target/release/Codeyard.app}
[ -f "$dmg" ] && [ -d "$portable" ] && [ -d "$app" ] || { echo 'Missing DMG, portable folder or app' >&2; exit 1; }
auth=(--keychain-profile "$NOTARY_PROFILE")
if [ -n "${NOTARY_KEYCHAIN:-}" ]; then auth+=(--keychain "$NOTARY_KEYCHAIN"); fi
logs=target/notary-logs
mkdir -p "$logs"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
submit() {
  local file=$1 name=$2 result=0 id status
  xcrun notarytool submit "$file" "${auth[@]}" --wait --timeout 20m --output-format json > "$logs/$name.json" || result=$?
  id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("id", ""))' "$logs/$name.json")
  status=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("status", ""))' "$logs/$name.json")
  if [ -n "$id" ]; then
    xcrun notarytool log "$id" "${auth[@]}" "$logs/$name-log.json" || echo "::warning::Unable to fetch notarization log for $name ($id)"
  fi
  if [ "$result" -ne 0 ] || [ "$status" != Accepted ]; then
    echo "::error::Notarization failed for $name: status=$status, submission=$id (see target/notary-logs)"
    return 1
  fi
  echo "Notarization accepted: $name ($id)"
}
codesign --verify --deep --strict "$app"
codesign --verify --strict "$dmg"
# DMG submission covers the app and its nested Mach-O agents. The portable binary has
# a separate signature, so submit that exact code too; Apple does not accept tar.gz.
submit "$dmg" dmg
ditto -c -k --keepParent "$portable" "$tmp/portable.zip"
submit "$tmp/portable.zip" portable
# Tickets for raw CLI binaries cannot be stapled. Their notarization is checked online.
for attempt in 1 2 3; do
  if xcrun stapler staple "$dmg"; then break; fi
  [ "$attempt" -lt 3 ] || exit 1
  sleep 10
done
xcrun stapler validate "$dmg"
codesign --verify --strict "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
spctl --assess --type execute --verbose=2 "$app"
echo "Notarized: $dmg and $portable; DMG ticket stapled and Gatekeeper checks passed"
