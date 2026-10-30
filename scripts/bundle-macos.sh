#!/usr/bin/env bash
# Build target/release/Codeyard.app and Codeyard.dmg, with the .icns generated from the SVG icon.
#   scripts/bundle-macos.sh
# macOS only (sips, iconutil, codesign, hdiutil). Apple silicon only; Intel is not a target.
# Signing is ad-hoc by default: no Apple Developer account needed, but the app only opens
# cleanly on the machine that built it. With a Developer ID:
#   SIGN_IDENTITY="Developer ID Application: Name (TEAMID)" scripts/bundle-macos.sh
# Other machines also need notarization (scripts/notarize-macos.sh); Gatekeeper blocks an
# unsigned download until the user picks right-click > Open.
# AGENTS_DIR=<dir>: the codeyard-agent-<triple> files in it go into the bundle under
# Contents/Resources/agents/; the app copies them to a server on first connect.
# Licence texts (scripts/licenses.sh) go into Contents/Resources/licenses/ and the DMG's Licenses/ folder.
# Icon source: crates/app/assets/macos/codeyard.svg (1024x1024, transparent corners).
set -euo pipefail
cd "$(dirname "$0")/.."
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
scripts/licenses.sh
cargo build --release -q -p app -p cyx -p ext-host
BIN=target/release/codeyard
VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c \
  'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="app"))')
APP=target/release/Codeyard.app
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# SVG -> 1024 px PNG (sips keeps the transparency), then the iconset sizes.
sips -s format png crates/app/assets/macos/codeyard.svg --out "$TMP/icon.png" >/dev/null
SET="$TMP/Codeyard.iconset"
mkdir "$SET"
for s in 16 32 128 256 512; do
  sips -z $s $s "$TMP/icon.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) "$TMP/icon.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o "$TMP/Codeyard.icns"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/codeyard"
# `cyx`, the extension authors' tool: beside the app so it finds it, and linked into PATH by
# "Extensions: Install cyx Command in PATH".
cp target/release/cyx "$APP/Contents/MacOS/cyx"
# `codeyard-exthost`, which runs code extensions: the only binary with wasmtime, and the only one with a
# hardened-runtime exception (crates/ext-host/exthost.entitlements).
cp target/release/codeyard-exthost "$APP/Contents/MacOS/codeyard-exthost"
if [ -n "${AGENTS_DIR:-}" ]; then
  # Resources/, not MacOS/: Linux binaries cannot be code-signed, so they are sealed as data.
  mkdir "$APP/Contents/Resources/agents"
  cp "$AGENTS_DIR"/codeyard-agent-* "$APP/Contents/Resources/agents/"
fi
cp "$TMP/Codeyard.icns" "$APP/Contents/Resources/Codeyard.icns"
# Licence texts: inside the bundle (before signing, so the seal covers them) and again at the DMG root.
scripts/licenses.sh copy "$APP/Contents/Resources/licenses"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Codeyard</string>
  <key>CFBundleDisplayName</key><string>Codeyard</string>
  <key>CFBundleIdentifier</key><string>com.codeyard.app</string>
  <key>CFBundleExecutable</key><string>codeyard</string>
  <key>CFBundleIconFile</key><string>Codeyard</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHumanReadableCopyright</key><string>Copyright © 2026 Codeyard contributors. MIT OR Apache-2.0.</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>com.codeyard.app</string>
      <key>CFBundleURLSchemes</key><array><string>codeyard</string></array>
    </dict>
  </array>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Text and source files</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.plain-text</string>
        <string>public.source-code</string>
        <string>public.json</string>
        <string>public.xml</string>
        <string>public.shell-script</string>
        <string>net.daringfireball.markdown</string>
        <string>public.data</string>
      </array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Folder</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key><array><string>public.folder</string></array>
    </dict>
  </array>
  <key>NSDesktopFolderUsageDescription</key><string>Codeyard opens and edits the files in folders you choose, including your Desktop.</string>
  <key>NSDocumentsFolderUsageDescription</key><string>Codeyard opens and edits the files in folders you choose, including your Documents.</string>
  <key>NSDownloadsFolderUsageDescription</key><string>Codeyard opens and edits the files in folders you choose, including your Downloads.</string>
  <key>NSRemovableVolumesUsageDescription</key><string>Codeyard opens and edits the files in folders you choose on external drives.</string>
  <key>NSNetworkVolumesUsageDescription</key><string>Codeyard opens and edits the files in folders you choose on network volumes.</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" >/dev/null

# Sign inside out: the bundled Mach-O agents first (`--deep` does not look into Resources/, and
# notarization rejects an unsigned Mach-O), then the app. Linux ELF agents count as data.
# --options runtime is required for notarization and harmless for an ad-hoc signature.
sign_args=(--force --options runtime --sign "${SIGN_IDENTITY:--}")
if [ -n "${SIGN_IDENTITY:-}" ]; then sign_args+=(--timestamp); fi
for agent in "$APP"/Contents/Resources/agents/codeyard-agent-*-apple-darwin; do
  [ -f "$agent" ] && codesign "${sign_args[@]}" "$agent"
done
# A nested executable is signed on its own before the bundle seal (notarization checks each one).
codesign "${sign_args[@]}" "$APP/Contents/MacOS/cyx"
codesign "${sign_args[@]}" --entitlements crates/ext-host/exthost.entitlements "$APP/Contents/MacOS/codeyard-exthost"
codesign "${sign_args[@]}" "$APP"
codesign --verify --deep --strict "$APP"

# DMG: the app plus an Applications link for drag-and-drop install.
DMG=target/release/Codeyard.dmg
STAGE="$TMP/dmg"
mkdir "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
scripts/licenses.sh copy "$STAGE/Licenses"
rm -f "$DMG"
hdiutil create -quiet -volname Codeyard -srcfolder "$STAGE" -ov -format UDZO "$DMG"
[ -n "${SIGN_IDENTITY:-}" ] && codesign --timestamp --sign "$SIGN_IDENTITY" "$DMG"
echo "$APP"
echo "$DMG"
