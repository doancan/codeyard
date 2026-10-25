# Platforms

| | macOS (Apple silicon) | Linux | Windows |
| --- | --- | --- | --- |
| App | released, daily use, all gates run | builds; the whole app test suite passed once in a Debian arm64 container (469 of 469 tests), not re-run since | builds natively (MSVC) and is checked by `scripts/check-windows.ps1` (library tests, path, terminal and resize regressions); ConPTY terminal, menu bar; SSH needs a key or ssh-agent |
| Agent (target of SSH) | `aarch64-apple-darwin` bundled | `x86_64` and `aarch64` musl bundled, ≤ 1600 KB | not a target |
| Installer | `.app` + DMG (`scripts/bundle-macos.sh`), not notarized | none yet | portable ZIP and Inno Setup installer (`scripts/bundle-windows.ps1`), unsigned |
| CI | `macos-latest` job runs the gates; all gates also run locally with `scripts/check.sh` | `linux` job (GPUI-free crates, musl agent size) is blocking; `linux-app` (whole workspace) is experimental and may fail | blocking `windows-2022` job running `scripts/check-windows.ps1` |

## macOS specifics

- **Bundle layout** (`scripts/bundle-macos.sh`): `Contents/MacOS/codeyard`, SSH agents in `Contents/Resources/agents/`
  (Linux ELF agents cannot be code-signed and are sealed as data; the app also still finds `MacOS/agents/` and an `agents/`
  folder next to a bare binary), icon, `Info.plist` (linted with `plutil`; document types for text, source code and
  folders; developer-tools category). The macOS Mach-O agent is signed explicitly (inside out) with the hardened runtime.
- **Signing and notarization.** Builds are ad-hoc signed (opens after right-click ▸ Open). `scripts/notarize-macos.sh`
  signs with a Developer ID, submits to `notarytool`, staples the DMG and runs `spctl`; it has **not** been run (there is no
  Apple Developer account yet). No entitlements are needed: the app is not sandboxed and loads no third-party libraries.
- **Finder integration**: *Open With*, Dock-icon drops and `open -a Codeyard file` work (verified with the real bundle).
- **Apple silicon only.** Intel Macs are not a target: there is no universal build and no Intel CI. `LSMinimumSystemVersion` and
  `MACOSX_DEPLOYMENT_TARGET` are 11.0 (the first macOS release for Apple silicon).

- The first start of each version touches Desktop, Documents and Downloads, so macOS asks in its own dialogs and lists Codeyard under
  Privacy & Security; the bundle carries usage descriptions for them, for removable and for network volumes. Codeyard also tries the
  permission database once, which makes the system add it (switched off) to Full Disk Access. *Help: Grant Full Disk Access…* opens that page.
- Metal toolchain (Xcode) is needed to build GPUI.
- The Dock icon is set at run time, also for a bare binary.

## Linux notes

- The vendored GPUI pulls in `xattr 0.2.3`, which uses `libc::ENOATTR` (missing on Linux); `vendor/xattr` patches it
  (`ENOATTR = ENODATA`) through `[patch.crates-io]`.
- Tests are written with macOS key names and the test harness maps them to the platform's equivalent
  (`cmd-s` → `ctrl-s`, `cmd-left` → `home`). Expectations that differ per platform are separated with `cfg!`.
- Terminal copy/paste is Ctrl+Shift+C / Ctrl+Shift+V (Ctrl+C must reach the program); ⌘G / ⇧⌘G (find next) are
  macOS-only, Linux and Windows use F3 / Shift+F3.
- To re-check Linux, build and run `cargo test --workspace` in a `rust:1-bookworm` container with GPUI's Linux
  libraries installed, using a separate `target` directory.

## Windows status

Windows x64 is built and released as a ZIP and a setup EXE (unsigned, so SmartScreen warns). Native pieces:

- The terminal uses ConPTY (Windows 10 1809 or newer); Git and language servers run without `/bin/sh`, resolve npm `.cmd`
  launchers and hide console windows; the application menu bar replaces the macOS global menu.
- `scripts/check-windows.ps1` runs the library tests, selected UI, path and keyboard regressions and the release build.
  GPUI needs `fxc.exe` from the Windows SDK.
- Native OpenSSH has no ControlMaster sockets: SSH needs a key or ssh-agent and an existing known-host entry (the password and
  host-key prompt bridge is Unix-only), and in-app port forwarding is unavailable; use `ssh -L` / `ssh -R` in a terminal.
- Not verified: real SSH authentication, mixed-DPI displays, physical keyboard and IME behaviour.
- `scripts/check-windows.sh` is the quick cross-compile check from macOS or Linux (`x86_64-pc-windows-gnu`); it runs nothing.

## Agent architectures

`uname -sm` on the server selects `codeyard-agent-<triple>`; an unknown architecture produces an explicit error.
Linux agents are cross-built with `cargo zigbuild --profile release-agent -p agent --target <triple>-unknown-linux-musl`.
