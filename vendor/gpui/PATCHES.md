# gpui 0.2.2, patched copy

Source: crates.io `gpui = "=0.2.2"` (Apache-2.0). Only what the build needs was copied (src, resources, build.rs).
It is used through `[patch.crates-io]` in the root `Cargo.toml`. The patches below must be re-applied on every GPUI upgrade.

| file | change | why |
| --- | --- | --- |
| build.rs, Cargo.toml | Windows build helper also runs on cross hosts; optional `GPUI_PRECOMPILED_SHADERS` supplies same-source FXC output | benchmark Linux → Windows MSVC while retaining manifest embedding |
| Cargo.toml | `[lints.rust] warnings = "allow"` | keep upstream warnings out of the build output |
| Cargo.toml | pin Taffy to `=0.9.2` | compatible patch moves layout storage to fixed `grid >=1.0.1`; older grid has integer-overflow UB ([GHSA-38c5-483c-4qqp](https://github.com/becheran/grid/security/advisories/GHSA-38c5-483c-4qqp)). `deny.toml` rejects vulnerable grid versions. |
| src/platform/mac/metal_renderer.rs | `set_maximum_drawable_count(3)` → `2` | saves ~16 MB of screen buffers per window on Retina.  Measured: budget table in docs/architecture.md (`scripts/memcheck.sh`). |
| src/window.rs | `DispatchEventResult` `pub(crate)` → `pub` | `Window::dispatch_event` is already pub, but its return type could not be named. Needed to send mouse events to a real window from inside the app (`--do drag:…`) |

`vendor/xattr` is patched the same way (`libc::ENOATTR` does not exist on Linux, `ENODATA` is used); it has nothing to do with
GPUI, but the same script checks it. Update the table and `scripts/check-gpui-patches.sh` together; the script runs from
`scripts/arch-check.sh` (and so in CI) and goes red when a patch is lost.

## GPUI upgrade checklist

1. **Get the new version.** Download and unpack the `gpui-X.Y.Z` source from crates.io
   (`curl -L https://crates.io/api/v1/crates/gpui/X.Y.Z/download | tar xz`); copy only what the build needs (`src`, `resources`,
   `build.rs`, `Cargo.toml`, licences) over `vendor/gpui`. Check with `git diff --stat vendor/gpui` that only the expected
   upstream changes came in.
2. **Pin the version.** `gpui = "=X.Y.Z"` in `crates/app/Cargo.toml` (two places: dependency and dev-dependency) and the version
   in the title of this file. The `[patch.crates-io]` path in the root `Cargo.toml` stays the same.
3. **Re-apply the patches** (table above):
   - `Cargo.toml`: add `warnings = "allow"` to a `[lints.rust]` section (upstream may already have
     `[lints.rust.unexpected_cfgs]`; add a separate `[lints.rust]` table or merge).
   - `Cargo.toml`: retain Taffy `=0.9.2` or verify an upgraded layout engine uses a fixed `grid` version.
   - `src/platform/mac/metal_renderer.rs`: `layer.set_maximum_drawable_count(3)` → `(2)`.
   - `src/window.rs`: `pub(crate) struct DispatchEventResult` → `pub struct` (with a doc line).
   - Run `scripts/check-gpui-patches.sh`: everything must say `ok`. If upstream now contains a patch itself or moved the code,
     update the pattern in the script and this table.
4. **Build and check** (merge only when all are green): `cargo build -p app`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, `scripts/arch-check.sh`, `scripts/docs-check.sh`.
5. **Dependency gates.** `cargo deny check` and `scripts/audit.sh`. For a new RustSec warning, add it to `.cargo/audit.toml` with a
   reason; delete IDs that no longer appear (`cargo audit` does not report unused exceptions, so review by hand).
   For a new licence, widen `[licenses].allow` in `deny.toml` deliberately. Update the "unmaintained crates that come only
   through GPUI" list in the `deny.toml` comment.
6. **Measure.** `scripts/memcheck.sh` (in particular, does the `set_maximum_drawable_count` patch still save ~16 MB) and one
   manual round in a real window: `cargo run --release -p app -- .`, the mouse event path with `--do drag:…`, VoiceOver
   (`docs/accessibility.md`).
7. **Document.** One line under *Unreleased* in `CHANGELOG.md`; update documents that mention the GPUI version
   (`grep -rn "0.2.2" docs README.md`).

If an upgrade shows that a patch is no longer needed: remove it from the table and the script first, then revert the code to upstream.

## Unmaintained-crate warnings (cargo-deny / cargo-audit)

`async-std`, `paste`, `proc-macro-error2`, `rustls-pemfile`, `rustybuzz`, `ttf-parser`, `instant`, `zbus_names`, `zvariant` and
`git2` (only through the `gpui_util` test helper) are transitive dependencies of GPUI; since we pin GPUI, the fix waits for an
upgrade. `deny.toml` makes them red only for direct dependencies (`unmaintained = "workspace"`); `cargo audit` reports all of
them, so `.cargo/audit.toml` excepts each ID individually, with a reason.

## What needs no patch

- **Accessibility (AccessKit, macOS):** no change in GPUI. `accesskit_macos::SubclassingAdapter` dynamically subclasses the
  GPUIView through `Window: HasWindowHandle` (AppKit `ns_view`); actions and activation reach GPUI through `AsyncApp`. App side:
  `crates/app/src/a11y.rs`, details: `docs/accessibility.md`. Note: `window_handle()` of the GPUI test window is
  `unimplemented!()`, so `a11y::attach` is skipped in tests.
