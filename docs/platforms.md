# Platforms

| | macOS (Apple silicon) | Linux | Windows |
| --- | --- | --- | --- |
| App | released, daily use, all gates run | builds; the full workspace is tested by blocking Linux CI; no released installer | builds natively (MSVC) and is checked by `scripts/check-windows.ps1` (library tests, path, terminal and resize regressions); ConPTY terminal, menu bar; SSH needs a key or ssh-agent |
| Agent (target of SSH) | `aarch64-apple-darwin` bundled | `x86_64` and `aarch64` musl bundled, ≤ 1664 KB | not a target |
| Installer | `.app` + DMG; Developer ID signing/notarization verified in release CI | none yet | portable ZIP and Inno Setup installer (`scripts/bundle-windows.ps1`), unsigned |
| CI | `macos-latest` job runs the gates; all gates also run locally with `scripts/check.sh` | `linux` job (GPUI-free crates, musl agent size) is blocking; `linux-app` (whole workspace/GPUI) is blocking after four consecutive successful runs | blocking `windows-2022` job running `scripts/check-windows.ps1` |

## macOS specifics

- **Bundle layout** (`scripts/bundle-macos.sh`): `Contents/MacOS/codeyard`, SSH agents in `Contents/Resources/agents/`
  (Linux ELF agents cannot be code-signed and are sealed as data; the app also still finds `MacOS/agents/` and an `agents/`
  folder next to a bare binary), icon, `Info.plist` (linted with `plutil`; document types for text, source code and
  folders; developer-tools category). The macOS Mach-O agent is signed explicitly (inside out) with the hardened runtime.
- **Signing and notarization.** GitHub tag releases use Developer ID signing and Apple notarization, with credentials in
  Actions Secrets. `scripts/notarize-macos.sh` notarizes the signed DMG and portable binaries, staples the DMG and runs
  `spctl`. A live signed dry run passed on 2026-10-26: Apple accepted both submissions and Gatekeeper reported
  `Notarized Developer ID` (see `releasing.md` for evidence). Ordinary dry runs remain ad-hoc signed; historical releases
  predating this change are unnotarized. No entitlements are currently configured: the app is not sandboxed and loads
  no third-party libraries.
- **Finder integration**: *Open With*, Dock-icon drops and `open -a Codeyard file` work (verified with the real bundle).
- **Apple silicon only.** Intel Macs are not a target: there is no universal build and no Intel CI. `LSMinimumSystemVersion` and
  `MACOSX_DEPLOYMENT_TARGET` are 11.0 (the first macOS release for Apple silicon).

- Codeyard asks for no file access at start, like VS Code. macOS asks, in its own dialog, the first time a folder on Desktop, Documents,
  Downloads, iCloud Drive, a removable or a network volume is opened; the bundle carries the usage descriptions for them. Full Disk Access
  is optional and cannot be requested by an app: *Help: Grant Full Disk Access…* opens the page and shows the app in Finder to drag in.
- Metal toolchain (Xcode) is needed to build GPUI.
- The Dock icon is set at run time, also for a bare binary.

## Linux notes

- The vendored GPUI pulls in `xattr 0.2.3`, which uses `libc::ENOATTR` (missing on Linux); `vendor/xattr` patches it
  (`ENOATTR = ENODATA`) through `[patch.crates-io]`.
- Tests are written with macOS key names and the test harness maps them to the platform's equivalent
  (`cmd-s` → `ctrl-s`, `cmd-left` → `home`). Expectations that differ per platform are separated with `cfg!`.
- Terminal copy/paste is Ctrl+Shift+C / Ctrl+Shift+V; Ctrl+C copies a selection or interrupts the program when nothing is selected. ⌘G / ⇧⌘G (find next) are
  macOS-only, Linux and Windows use F3 / Shift+F3.
- To re-check Linux, build and run `cargo test --workspace` in a `rust:1-bookworm` container with GPUI's Linux
  libraries installed, using a separate `target` directory.

## Windows status

Windows x64 is built and released as a ZIP and a setup EXE (unsigned, so SmartScreen warns). Native pieces:

- The terminal uses ConPTY (Windows 10 1809 or newer); Git and language servers run without `/bin/sh`, resolve npm `.cmd`
  launchers and hide console windows; the application menu bar replaces the macOS global menu.
- Accessibility: the AccessKit tree reaches UI Automation (NVDA, JAWS, Narrator) through `accesskit_windows`; the window
  opens hidden and is shown once the adapter is attached. Not yet tried with a screen reader (see `accessibility.md`).
- `scripts/check-windows.ps1` runs the library tests, selected UI, path and keyboard regressions and the release build.
  GPUI needs `fxc.exe` from the Windows SDK.
- Terminals of a local folder run in the terminal daemon, as on macOS and Linux. They are taken back after Codeyard
  quits, crashes or is killed (`terminal.persistentSessionTimeout`). The daemon listens on a named pipe that only the
  current user can open (a protected DACL with the user's SID alone; remote clients rejected) and owns the ConPTY
  sessions. CI checks it with real processes (`agent/src/daemon_windows_tests.rs`): the same shell is taken back with
  its token, a wrong token is refused, a kept session ends with its shell after its timeout, and the pipe's DACL is
  checked.
- Tasks: each task runs in a kill-on-close job object. CI runs these checks with real processes
  (`agent/src/proc_windows_tests.rs`):
  - Stop ends the task's leader and a grandchild that holds the output pipe, also after the leader has exited.
  - Killing the process that owns the task ends both, because the system closes the job handle.

  The task runner's quoting (`app/src/task_run_tests.rs`, the `windows_task_arguments_*` tests) is checked by running
  Python as the program through a process task, cmd.exe and PowerShell. The values include spaces, quotes, `%VAR%`, `^`,
  `&`, `|`, `<`, `>`, Unicode, trailing backslashes and an empty argument, and the test compares the argument list the
  program received. cmd.exe words keep `%` out of the quotes so a `%NAME%` value is not expanded.
  Windows PowerShell 5.1 cannot pass an empty argument, an embedded quote or a trailing backslash to a program
  unchanged; PowerShell 7 (`pwsh`) can.
- `./scripts/build-windows.ps1` builds a standalone EXE with Linux x64 and ARM64 SSH agents embedded; use
  `-Profile Release` for an optimized build. No separate `agents/` folder is needed. The packaging check moves only
  the EXE to an empty directory and verifies both embedded agents. Release CI also embeds its supplied macOS agents.
- Native OpenSSH has no ControlMaster sockets: SSH needs a key or ssh-agent and an existing known-host entry (the password and
  host-key prompt bridge is Unix-only), and in-app port forwarding is unavailable; use `ssh -L` / `ssh -R` in a terminal.
- Rust debugging with the MSVC toolchain (`*-pc-windows-msvc`): lldb reads the PDB debug information there, which the
  toolchain's lldb formatters are not made for, so they are not loaded and `String`, `Vec` and `HashMap` show their fields.
  The `*-pc-windows-gnu` toolchains get them as on other systems.
- Extensions: declarative extensions, the Extensions view and `cyx` work as on macOS, and the installer puts `cyx.exe` beside
  Codeyard. *Extensions: Install cyx Command in PATH* names that folder instead of linking (the PATH is the user's to edit).
  The extension performance rows only warn on Windows until there is a Windows reference machine (`plugin-architecture.md` §15).
- Not verified: real SSH authentication, mixed-DPI displays, physical keyboard and IME behaviour.
- `scripts/check-windows.sh` is the quick cross-compile check from macOS or Linux (`x86_64-pc-windows-gnu`); it runs nothing.

## Agent architectures

`uname -sm` on the server selects `codeyard-agent-<triple>`; an unknown architecture produces an explicit error.
Linux agents are cross-built with `cargo zigbuild --profile release-agent -p agent --target <triple>-unknown-linux-musl`.
