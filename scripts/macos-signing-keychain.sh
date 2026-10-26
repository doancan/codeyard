#!/usr/bin/env bash
# GitHub-hosted macOS runner only. Private files stay under RUNNER_TEMP, never the checkout.
set +x
set -euo pipefail
: "${RUNNER_TEMP:?GitHub runner temporary directory is required}"
work="$RUNNER_TEMP/codeyard-apple-signing"
case "${1:-}" in
  cleanup)
    result=0
    if [ -f "$work/search-list.json" ]; then
      python3 - "$work/search-list.json" <<'PY' || result=$?
import json, subprocess, sys
subprocess.run(['security', 'list-keychains', '-d', 'user', '-s', *json.load(open(sys.argv[1]))], check=True)
PY
    fi
    if [ -e "$work/signing.keychain-db" ]; then
      security delete-keychain "$work/signing.keychain-db" || result=$?
    fi
    rm -rf "$work"
    exit "$result"
    ;;
  setup) ;;
  *) echo 'usage: macos-signing-keychain.sh setup|cleanup' >&2; exit 2 ;;
esac
: "${GITHUB_ENV:?GitHub environment file is required}"
for name in MACOS_CERTIFICATE_P12 MACOS_CERTIFICATE_PASSWORD APPLE_ID APPLE_APP_SPECIFIC_PASSWORD APPLE_TEAM_ID; do
  if [ -z "${!name:-}" ]; then echo "::error::Missing GitHub Actions secret: $name"; exit 1; fi
done
umask 077
mkdir "$work"
trap 'rm -f "$work/certificate.p12"' EXIT
security list-keychains -d user | python3 -c 'import json,shlex,sys; json.dump(shlex.split(sys.stdin.read()),sys.stdout)' > "$work/search-list.json"
python3 - "$work/certificate.p12" <<'PY'
import base64, os, pathlib, sys
pathlib.Path(sys.argv[1]).write_bytes(base64.b64decode(''.join(os.environ['MACOS_CERTIFICATE_P12'].split()), validate=True))
PY
password="$(openssl rand -hex 32)"
keychain="$work/signing.keychain-db"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 7200 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$work/certificate.p12" -k "$keychain" -P "$MACOS_CERTIFICATE_PASSWORD" -T /usr/bin/codesign > /dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" > /dev/null
python3 - "$work/search-list.json" "$keychain" <<'PY'
import json, subprocess, sys
subprocess.run(['security','list-keychains','-d','user','-s',sys.argv[2],*json.load(open(sys.argv[1]))],check=True)
PY
identity="$(security find-identity -v -p codesigning "$keychain" | python3 -c '
import os,re,sys
matches=re.findall(r"([A-F0-9]{40}) \"Developer ID Application: .*? \(([A-Z0-9]+)\)\"",sys.stdin.read())
valid=[fingerprint for fingerprint,team in matches if team==os.environ["APPLE_TEAM_ID"]]
if len(valid)!=1: sys.exit("Expected exactly one valid Developer ID Application identity matching APPLE_TEAM_ID")
print(valid[0])
')"
xcrun notarytool store-credentials codeyard-notary --keychain "$keychain" \
  --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_SPECIFIC_PASSWORD" > /dev/null
printf 'SIGN_IDENTITY=%s\nNOTARY_PROFILE=codeyard-notary\nNOTARY_KEYCHAIN=%s\n' "$identity" "$keychain" >> "$GITHUB_ENV"
