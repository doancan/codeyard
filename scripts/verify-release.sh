#!/usr/bin/env bash
# Check a Codeyard release: the signature on SHA256SUMS.txt, then the hash of every file.
#   scripts/verify-release.sh <tag>            download the release with `gh` and check it (e.g. v0.2.0)
#   scripts/verify-release.sh --dir <path>     check a folder you already downloaded
#   scripts/verify-release.sh --self-test      sign and check a made-up release with a throw-away key (CI runs this)
# Options: --key <pem>   public key to trust instead of scripts/update-public-key.pem (used by the tests)
#          --repo <o/r>  repository for <tag> (default doancan/codeyard)
# Needs OpenSSL with Ed25519 (`pkeyutl -rawin`) and sha256sum or shasum. Exit 0 only when the signature is valid, the list is well formed,
# every listed file matches its hash and no file in the folder is missing from the list.
#
# The list must satisfy the rules of the in-app updater (crates/app/src/update.rs, `Sums::parse`): the same ones, so that a
# release this script accepts is one the app accepts and the other way round. Every line is `<64 lowercase hex>`, TWO spaces and
# a name of [A-Za-z0-9._-] (no `*` binary marker, no CRLF, no uppercase hex); no name twice; the versions in the names of
# `Codeyard-<x.y.z>-<macos-arm64.dmg|macos-arm64.tar.gz|windows-x64-setup.exe|windows-x64.zip>` (the first letter may be lower
# case) are all one version, digits only, no leading zeros; at least one installer (the .dmg or the setup .exe, named
# `Codeyard-<version>-...`) and RELEASE_NOTES.md are listed.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
key="$here/update-public-key.pem" repo="doancan/codeyard" dir="" tag="" self_test=0
while [ $# -gt 0 ]; do
  case "$1" in
    --dir) dir="${2:?--dir needs a path}"; shift 2 ;;
    --key) key="${2:?--key needs a path}"; shift 2 ;;
    --repo) repo="${2:?--repo needs owner/name}"; shift 2 ;;
    --self-test) self_test=1; shift ;;
    -*) echo "verify-release: unknown option $1" >&2; exit 2 ;;
    *) tag="$1"; shift ;;
  esac
done

sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
fail() { echo "FAIL: $*" >&2; return 1; }

# x.y.z of digits only, no leading zeros, each part within 32 bits (what the app parses).
valid_version() {
  [[ "$1" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || return 1
  local c
  for c in "${BASH_REMATCH[@]:1}"; do [ "${#c}" -le 10 ] && [ "$c" -le 4294967295 ] || return 1; done
}

# check_dir <dir> <pubkey>: prints one line per check, returns non-zero on the first failure.
check_dir() {
  local d="$1" pub="$2" sums="$1/SHA256SUMS.txt" sig="$1/SHA256SUMS.txt.sig"
  openssl pkeyutl -help 2>&1 | grep -- '-rawin' >/dev/null || { fail "openssl pkeyutl -rawin is required (found: $(openssl version))"; return 1; }
  [ -s "$sums" ] || { fail "SHA256SUMS.txt is missing"; return 1; }
  [ -f "$sig" ] || { fail "SHA256SUMS.txt.sig is missing: this release is not signed"; return 1; }
  [ "$(wc -c < "$sig" | tr -d ' ')" = 64 ] || { fail "SHA256SUMS.txt.sig is not 64 bytes"; return 1; }
  openssl pkeyutl -verify -pubin -inkey "$pub" -rawin -in "$sums" -sigfile "$sig" >/dev/null 2>&1 \
    || { fail "the signature on SHA256SUMS.txt is not valid for $pub"; return 1; }
  echo "signature ok"
  local listed="" line h n count=0 version="" installers=0 notes=0 v p sfx
  while IFS= read -r line || [ -n "$line" ]; do
    [ -n "$line" ] || continue
    [[ "$line" =~ ^([0-9a-f]{64})\ \ ([A-Za-z0-9._-]+)$ ]] \
      || { fail "malformed line in SHA256SUMS.txt (want <64 lowercase hex>, two spaces, a name of A-Za-z0-9._-): $line"; return 1; }
    h="${BASH_REMATCH[1]}" n="${BASH_REMATCH[2]}"
    if [ -n "$listed" ] && printf '%s\n' "$listed" | grep -Fxq -- "$n"; then fail "$n is listed twice in SHA256SUMS.txt"; return 1; fi
    for sfx in macos-arm64.dmg macos-arm64.tar.gz windows-x64-setup.exe windows-x64.zip; do
      for p in Codeyard- codeyard-; do
        [[ "$n" == "$p"*"-$sfx" ]] || continue
        v="${n#"$p"}"; v="${v%"-$sfx"}"
        valid_version "$v" || { fail "unusable version '$v' in $n (want x.y.z, digits only, no leading zeros)"; return 1; }
        [ -z "$version" ] || [ "$version" = "$v" ] || { fail "mixed versions in SHA256SUMS.txt: $version and $v ($n)"; return 1; }
        version="$v"
        case "$p$sfx" in Codeyard-macos-arm64.dmg|Codeyard-windows-x64-setup.exe) installers=$((installers + 1)) ;; esac
      done
    done
    [ "$n" != RELEASE_NOTES.md ] || notes=1
    [ -f "$d/$n" ] || { fail "$n is listed but missing"; return 1; }
    [ "$(sha256 "$d/$n")" = "$h" ] || { fail "$n does not match its hash"; return 1; }
    echo "ok  $n"; listed="$listed
$n"; count=$((count + 1))
  done < "$sums"
  [ "$count" -gt 0 ] || { fail "SHA256SUMS.txt lists no files"; return 1; }
  [ -n "$version" ] || { fail "SHA256SUMS.txt lists no Codeyard-<version>-<platform> file"; return 1; }
  [ "$installers" -gt 0 ] || { fail "SHA256SUMS.txt lists no installer (Codeyard-$version-macos-arm64.dmg or -windows-x64-setup.exe)"; return 1; }
  [ "$notes" = 1 ] || { fail "RELEASE_NOTES.md is not listed in SHA256SUMS.txt (the app shows it, so it must be signed)"; return 1; }
  local f b
  for f in "$d"/*; do
    b="$(basename "$f")"
    case "$b" in SHA256SUMS.txt|SHA256SUMS.txt.sig) continue ;; esac
    printf '%s\n' "$listed" | grep -Fxq -- "$b" || { fail "$b is in the release but not in SHA256SUMS.txt"; return 1; }
  done
  echo "verified $count files"
}

self_test() {
  local pass=0 bad=0
  t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
  openssl genpkey -algorithm ed25519 -out "$t/k1.pem" 2>/dev/null
  openssl pkey -in "$t/k1.pem" -pubout -out "$t/k1.pub" 2>/dev/null
  openssl genpkey -algorithm ed25519 -out "$t/k2.pem" 2>/dev/null
  mkdir "$t/rel"
  printf 'dmg\n' > "$t/rel/Codeyard-9.9.9-macos-arm64.dmg"
  printf 'tar\n' > "$t/rel/codeyard-9.9.9-macos-arm64.tar.gz"
  printf 'zip\n' > "$t/rel/codeyard-9.9.9-windows-x64.zip"
  printf 'exe\n' > "$t/rel/Codeyard-9.9.9-windows-x64-setup.exe"
  printf '## [9.9.9]\n- test\n' > "$t/rel/RELEASE_NOTES.md"
  (cd "$t/rel" && for f in *; do printf "%s  %s\n" "$(sha256 "$f")" "$f"; done > ../sums && mv ../sums SHA256SUMS.txt)
  "$here/sign-release.sh" "$t/rel" --key-file "$t/k1.pem" --pubkey "$t/k1.pub" >/dev/null
  # also the env-var path the workflow uses
  mkdir "$t/envrel"; cp "$t/rel/SHA256SUMS.txt" "$t/envrel/"
  UPDATE_SIGNING_KEY="$(cat "$t/k1.pem")" "$here/sign-release.sh" "$t/envrel" --pubkey "$t/k1.pub" >/dev/null
  cmp -s "$t/rel/SHA256SUMS.txt.sig" "$t/envrel/SHA256SUMS.txt.sig" || { echo "FAIL: env and file signing differ (Ed25519 is deterministic)"; return 1; }

  expect() { # expect pass|fail <name> <dir> [pubkey] [pattern the failure message must contain]
    local want="$1" name="$2" d="$3" pub="${4:-$t/k1.pub}" pattern="${5:-}" rc=0
    check_dir "$d" "$pub" >"$t/out" 2>&1 || rc=$?
    if [ -n "$pattern" ] && ! grep -q -- "$pattern" "$t/out"; then
      echo "self-test BAD: $name (failed, but not with '$pattern')"; cat "$t/out"; bad=$((bad + 1))
    elif { [ "$want" = pass ] && [ "$rc" = 0 ]; } || { [ "$want" = fail ] && [ "$rc" != 0 ]; }; then
      echo "self-test ok: $name ($(grep -m1 FAIL "$t/out" || echo accepted))"; pass=$((pass + 1))
    else
      echo "self-test BAD: $name (wanted $want, exit $rc)"; cat "$t/out"; bad=$((bad + 1))
    fi
  }
  fresh() { rm -rf "$t/c"; cp -R "$t/rel" "$t/c"; }
  flip() { printf '\xff' | dd of="$1" bs=1 seek=0 count=1 conv=notrunc 2>/dev/null; }

  expect pass "good release" "$t/rel"
  fresh; flip "$t/c/Codeyard-9.9.9-macos-arm64.dmg"; expect fail "one byte changed in an asset" "$t/c"
  fresh; flip "$t/c/RELEASE_NOTES.md"; expect fail "one byte changed in RELEASE_NOTES.md" "$t/c"
  fresh; flip "$t/c/SHA256SUMS.txt"; expect fail "one byte changed in SHA256SUMS.txt" "$t/c"
  fresh; rm "$t/c/SHA256SUMS.txt.sig"; expect fail "missing signature" "$t/c"
  fresh; head -c 63 "$t/rel/SHA256SUMS.txt.sig" > "$t/c/SHA256SUMS.txt.sig"; expect fail "truncated signature" "$t/c"
  fresh; rm "$t/c/SHA256SUMS.txt.sig"; openssl pkeyutl -sign -inkey "$t/k2.pem" -rawin -in "$t/c/SHA256SUMS.txt" -out "$t/c/SHA256SUMS.txt.sig"
  expect fail "signed by a different key" "$t/c"
  fresh; rm "$t/c/codeyard-9.9.9-windows-x64.zip"; expect fail "listed asset missing" "$t/c"
  fresh; printf 'x\n' > "$t/c/extra.bin"; expect fail "asset not listed in SHA256SUMS.txt" "$t/c"
  # a signed list that points outside the folder must be refused even though the signature is valid
  fresh; printf '%s  ../k1.pem\n' "$(sha256 "$t/k1.pem")" >> "$t/c/SHA256SUMS.txt"
  rm "$t/c/SHA256SUMS.txt.sig"; openssl pkeyutl -sign -inkey "$t/k1.pem" -rawin -in "$t/c/SHA256SUMS.txt" -out "$t/c/SHA256SUMS.txt.sig"
  expect fail "signed list with a path outside the folder" "$t/c"
  # The rules of the app's parser. Each case writes a list that is validly SIGNED but wrong, so only the rules can refuse it.
  resign() { rm -f "$t/c/SHA256SUMS.txt.sig"; openssl pkeyutl -sign -inkey "$t/k1.pem" -rawin -in "$t/c/SHA256SUMS.txt" -out "$t/c/SHA256SUMS.txt.sig"; }
  relist() { (cd "$t/c" && for f in *; do case "$f" in SHA256SUMS.txt|SHA256SUMS.txt.sig) continue ;; esac; printf "%s  %s\n" "$(sha256 "$f")" "$f"; done > ../sums && mv ../sums SHA256SUMS.txt); resign; }
  fresh; mv "$t/c/codeyard-9.9.9-windows-x64.zip" "$t/c/codeyard-9.9.8-windows-x64.zip"; relist
  expect fail "assets of two versions" "$t/c" "$t/k1.pub" "mixed versions"
  fresh; mv "$t/c/Codeyard-9.9.9-macos-arm64.dmg" "$t/c/Codeyard-09.9.9-macos-arm64.dmg"; relist
  expect fail "a version with a leading zero" "$t/c" "$t/k1.pub" "unusable version"
  fresh; sed -i.bak 's/^\([0-9a-f]\{64\}\)  \(RELEASE_NOTES.md\)$/\1 *\2/' "$t/c/SHA256SUMS.txt"; rm "$t/c/SHA256SUMS.txt.bak"; resign
  expect fail "a binary-mode marker (*)" "$t/c" "$t/k1.pub" "malformed line"
  fresh; sed -i.bak 's/^\([0-9a-f]\{64\}\)  \(RELEASE_NOTES.md\)$/\1 \2/' "$t/c/SHA256SUMS.txt"; rm "$t/c/SHA256SUMS.txt.bak"; resign
  expect fail "a single space" "$t/c" "$t/k1.pub" "malformed line"
  fresh; sed -i.bak 's/^\([0-9a-f]\{64\}\)  \(RELEASE_NOTES.md\)$/\1   \2/' "$t/c/SHA256SUMS.txt"; rm "$t/c/SHA256SUMS.txt.bak"; resign
  expect fail "three spaces" "$t/c" "$t/k1.pub" "malformed line"
  fresh; awk '{ print toupper($1) "  " $2 }' "$t/rel/SHA256SUMS.txt" > "$t/c/SHA256SUMS.txt"; resign
  expect fail "upper-case hex (the app wants lower case)" "$t/c" "$t/k1.pub" "malformed line"
  fresh; sed 's/$/\r/' "$t/rel/SHA256SUMS.txt" > "$t/c/SHA256SUMS.txt"; resign
  expect fail "CRLF line ends" "$t/c" "$t/k1.pub" "malformed line"
  fresh; head -1 "$t/rel/SHA256SUMS.txt" >> "$t/c/SHA256SUMS.txt"; resign
  expect fail "a name listed twice" "$t/c" "$t/k1.pub" "listed twice"
  fresh; printf 'x\n' > "$t/c/my file.txt"; relist
  expect fail "a name with a space" "$t/c" "$t/k1.pub" "malformed line"
  fresh; rm "$t/c/Codeyard-9.9.9-macos-arm64.dmg" "$t/c/Codeyard-9.9.9-windows-x64-setup.exe"; relist
  expect fail "no installer of any platform" "$t/c" "$t/k1.pub" "no installer"
  fresh; rm "$t/c/RELEASE_NOTES.md"; relist
  expect fail "no release notes" "$t/c" "$t/k1.pub" "RELEASE_NOTES.md is not listed"
  fresh; rm "$t/c/Codeyard-9.9.9-macos-arm64.dmg"; relist
  expect pass "an installer for one platform is enough" "$t/c"
  fresh; relist
  expect pass "a list the script wrote itself" "$t/c"
  echo "self-test: $pass passed, $bad failed"
  [ "$bad" = 0 ]
}

if [ "$self_test" = 1 ]; then self_test; exit $?; fi
if [ -n "$dir" ] && [ -n "$tag" ]; then echo "verify-release: give a tag or --dir, not both" >&2; exit 2; fi
if [ -z "$dir" ]; then
  [ -n "$tag" ] || { echo "usage: verify-release.sh <tag> | --dir <path> | --self-test  [--key <pem>] [--repo owner/name]" >&2; exit 2; }
  command -v gh >/dev/null 2>&1 || { echo "verify-release: gh is required to download a tag (or download the files and use --dir)" >&2; exit 2; }
  dir="$(mktemp -d)"; trap 'rm -rf "$dir"' EXIT
  gh release download "$tag" --repo "$repo" --dir "$dir" --pattern '*'
fi
check_dir "$dir" "$key"
