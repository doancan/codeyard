#!/usr/bin/env bash
# Licence texts that every release artifact must ship, and the helpers that copy them.
#   scripts/licenses.sh                 fail if a listed file is missing
#   scripts/licenses.sh copy <dir>      check, then copy them into <dir> (created if needed)
# The bundle scripts (macOS, Windows) call `copy`; installer/windows/codeyard.iss lists the same files.
set -euo pipefail
here=$PWD
cd "$(dirname "$0")/.."

# source path -> name inside the licences folder (directories keep their contents)
FILES=(
  "LICENSE-MIT LICENSE-MIT"
  "LICENSE-APACHE LICENSE-APACHE"
  "THIRD_PARTY_NOTICES.md THIRD_PARTY_NOTICES.md"
  "THIRD_PARTY_CRATES.md THIRD_PARTY_CRATES.md"
  "crates/app/assets/icons/LICENSE icons-LICENSE"
  "crates/app/assets/TABLER-LICENSE tabler-icons-LICENSE"
  "crates/syntax/grammars/dockerfile/LICENSE dockerfile-grammar-LICENSE"
  "vendor/gpui/LICENSE-APACHE gpui-LICENSE-APACHE"
  "vendor/xattr/LICENSE-MIT xattr-LICENSE-MIT"
  "vendor/xattr/LICENSE-APACHE xattr-LICENSE-APACHE"
  "crates/spell/dicts/en/license spell-dict-en-license"
  "crates/syntax/LICENSES syntax-LICENSES"
  "crates/app/assets/file-icons/LICENSES file-icons-LICENSES"
)

missing=0
for entry in "${FILES[@]}"; do
  src=${entry%% *}
  if [ ! -e "$src" ]; then echo "licenses: missing $src" >&2; missing=1
  elif [ -f "$src" ] && [ ! -s "$src" ]; then echo "licenses: $src is empty" >&2; missing=1; fi
done
[ -d crates/syntax/LICENSES ] && [ -z "$(ls -A crates/syntax/LICENSES)" ] && { echo "licenses: crates/syntax/LICENSES is empty" >&2; missing=1; }
# The Windows script and the installer keep their own copy of the list. Each entry must appear in both with the same
# source path and target name (whole entries, not substrings: `LICENSE-MIT` is also a substring of `vendor\xattr\LICENSE-MIT`).
for entry in "${FILES[@]}"; do
  src=${entry%% *} name=${entry#* }; win=${src//\//\\}
  if ! grep -qF -- "'$win' = '$name'" scripts/bundle-windows.ps1; then
    echo "licenses: scripts/bundle-windows.ps1 does not map $src -> $name" >&2; missing=1
  fi
  if [ -d "$src" ]; then tok="Source: \"..\\..\\$win\\*\";"; want="DestDir: \"{app}\\licenses\\$name\""
  else tok="Source: \"..\\..\\$win\";"; want="DestDir: \"{app}\\licenses\""
    [ "$name" = "${src##*/}" ] || want="DestName: \"$name\""; fi
  line=$(grep -F -- "$tok" installer/windows/codeyard.iss | head -1 || true)
  case "$line" in
    "") echo "licenses: installer/windows/codeyard.iss does not list $src" >&2; missing=1 ;;
    *"$want"*) if [ ! -d "$src" ] && [ "$name" = "${src##*/}" ] && [[ "$line" == *DestName* ]]; then
         echo "licenses: installer/windows/codeyard.iss renames $src, but the list keeps its name ($name)" >&2; missing=1; fi ;;
    *) echo "licenses: installer/windows/codeyard.iss: $src is not installed as $name (expected $want)" >&2; missing=1 ;;
  esac
done
[ "$missing" = 0 ] || exit 1

case "${1:-}" in
  "") echo "licenses: ok" ;;
  copy)
    dest=${2:?usage: licenses.sh copy <dir>}
    case "$dest" in /*) ;; *) dest=$here/$dest ;; esac
    mkdir -p "$dest"
    for entry in "${FILES[@]}"; do
      src=${entry%% *} name=${entry#* }
      rm -rf "${dest:?}/$name"
      cp -R "$src" "$dest/$name"
    done
    ;;
  *) echo "usage: licenses.sh [copy <dir>]" >&2; exit 2 ;;
esac
