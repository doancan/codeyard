#!/usr/bin/env bash
# Check that the patches under vendor/ are still in place (table in vendor/gpui/PATCHES.md).
#   scripts/check-gpui-patches.sh
# Goes red when a GPUI upgrade drops a patch. A new patch gets an entry here and in PATCHES.md.
set -euo pipefail
cd "$(dirname "$0")/.."
fail=0
need() { # file pattern description
  if grep -Eq -- "$2" "$1"; then echo "ok   $1: $3"; else echo "FAIL $1: missing patch '$3' (pattern: $2)"; fail=1; fi
}
absent() { # file pattern description
  if grep -Eq -- "$2" "$1"; then echo "FAIL $1: unpatched '$3' is back (pattern: $2)"; fail=1; else echo "ok   $1: no $3"; fi
}
need vendor/gpui/Cargo.toml '^\[lints\.rust\]$' "[lints.rust] section"
need vendor/gpui/Cargo.toml '^warnings = "allow"$' "warnings = \"allow\""
need vendor/gpui/Cargo.toml '^version = "=0\.9\.2"$' "Taffy patch with fixed grid dependency"
need vendor/gpui/src/platform.rs 'crate::swap_rgba_pa_to_bgra\(pixel\)' "SVG image channels and alpha match raster images"
need vendor/gpui/src/elements/text.rs 'let Some\(mouse_down_index\) = mouse_down_on_up.take\(\)' "link click handlers share press state and clear cancellation"
need vendor/gpui/src/platform/windows/clipboard.rs 'format == CF_DIB\.0 as u32 \|\| format == CF_DIBV5\.0 as u32' "Windows DIB clipboard image formats"
need vendor/gpui/src/platform/windows/clipboard.rs 'fn dib_to_png\(' "bounded Windows DIB to PNG conversion"
need vendor/gpui/src/platform/mac/metal_renderer.rs 'set_maximum_drawable_count\(2\)' "drawable count 2"
absent vendor/gpui/src/platform/mac/metal_renderer.rs 'set_maximum_drawable_count\(3\)' "drawable count 3"
need vendor/gpui/src/window.rs '^pub struct DispatchEventResult' "pub DispatchEventResult"
need vendor/gpui/src/ui_scale.rs 'pub fn to_logical\(self, ui_scale: f32\)' "interface scale input conversion"
need vendor/gpui/src/app.rs 'pub fn set_ui_scale\(&mut self, scale: f32\)' "App::set_ui_scale"
need vendor/gpui/src/window.rs 'let event = event.to_logical\(window.ui_scale\);' "platform input converted to logical pixels"
need vendor/gpui/src/window.rs 'self.sync_ui_scale\(cx\);' "interface scale applied before each frame"
need vendor/gpui/src/window.rs 'platform_window.scale_factor\(\) \* self.ui_scale' "display scale combined with interface scale"
need vendor/gpui/src/platform.rs 'ui_bounds_to_platform\(bounds, window.ui_scale\(\)\)' "IME bounds in platform points"
absent vendor/gpui/src/window.rs '^pub\(crate\) struct DispatchEventResult' "pub(crate) DispatchEventResult"
need vendor/xattr/src/sys/mod.rs 'libc::ENODATA' "Linux ENODATA instead of ENOATTR (xattr)"
# The root Cargo.toml uses the patches, and the version is pinned.
need Cargo.toml '^gpui = \{ path = "vendor/gpui" \}' "[patch.crates-io] gpui"
need Cargo.toml '^xattr = \{ path = "vendor/xattr" \}' "[patch.crates-io] xattr"
want=$(sed -n 's/^version = "\(.*\)"/\1/p' vendor/gpui/Cargo.toml | head -1)
need vendor/gpui/PATCHES.md "^# gpui $want," "PATCHES.md title names gpui $want"
need vendor/gpui/build.rs 'GPUI_PRECOMPILED_SHADERS' "precompiled Windows shaders for cross builds"
need vendor/gpui/Cargo.toml '^\[build-dependencies\.embed-resource\]$' "resource compiler available on cross hosts"
exit $fail
