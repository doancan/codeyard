# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-10-25

### Added

- **Updates.** *Help: Check for Updates…* (also in the Help menu, and under the app menu on macOS) opens a dialog that checks, shows the
  release notes, downloads with progress, verifies against the signed checksums, installs and restarts through the normal quit path
  (your windows reopen, with their tabs and unsaved work, as far as `window.restoreSession` and `files.hotExit` allow; the dialog says what will come back). *Later*, *Skip This Version* and *Release Page* are there too, an update on offer or on its way shows in the
  status bar, and a copy that cannot replace itself offers the download page with the reason. The new setting `update.check`
  (`ask`, `daily`, `manual`; your own settings only) decides whether Codeyard looks by itself: the default `ask` checks nothing and, once,
  asks whether to check daily. `daily` sends one request a day to github.com for the signed checksum list and nothing else.

### Changed

- The About dialog shows the version and platform only.
- The Full Disk Access dialog explains that the app has to be added to the list by hand, can show Codeyard in Finder, and
  asks again once after each update while access is missing (a release is a new app to macOS).

### Fixed

- SSH connections no longer fail when the home folder path is long (the control socket path exceeded the Unix socket limit).

## [0.1.0] - 2026-10-25

First public release: a small native code editor for local folders and remote servers over SSH, written in Rust on GPUI.
No extensions, no telemetry, no account.

### Added

**Editor**
- **Editing.** Multiple cursors, column (box) selection, grouped undo, word and line operations, case transforms, linked tag editing,
  Emmet, snippets (built-in sets and VS Code format files), EditorConfig, format on save, on paste and on type, and code actions on save.
- **Navigation.** Split editor groups, preview and pinned tabs, back/forward, folding (levels, recursive), sticky scroll, minimap,
  breadcrumbs, outline, quick open, command palette, workspace symbols, and multi-root workspaces (`.code-workspace`).
- **Find and replace** in the file and across files, with regex, whole word, smart case and preserve case; results stream in and open as a tab.
- **Syntax highlighting** for more than 20 languages (tree-sitter), bracket pair colours, Unicode highlight, colour literals
  (hex, `rgb()`, `hsl()`, names) with a picker, and character-level diffs.
- **Files.** Non-UTF-8 encodings (UTF-16, Windows-1254, ISO-8859-1/9) saved back unchanged, image preview, a hex view, log follow (`tail -f`)
  with filtering, and files over 32 MB in a chunked read-only view (a 554 MB file opens at about 70 MB of memory).
- **Session.** Projects, favourites, bookmarks, session restore and hot exit (tabs, cursors and unsaved edits survive quit and crash),
  a welcome page, a Settings page and a Keyboard Shortcuts page, with `settings.jsonc` and `keybindings.jsonc` reloaded live.
- **Terminal and tasks.** Integrated terminal (ConPTY on Windows), tasks with variables and dependencies, problems parsed from task output,
  and tests run from the editor for Rust, Go, JS/TS and Python.
- **Import** of VS Code settings, shortcuts and themes.

**Remote (SSH)**
- Uses your system `ssh` and `~/.ssh/config`; passwords, passphrases, one-time codes and host keys are asked in the window.
- A small static agent on the server handles search, terminals, file watching, git and language servers; only results cross the network.
- Sessions survive dropped connections and reconnect automatically, with a heartbeat for unresponsive agents and a clear message and
  **Reconnect** button when it gives up. Unsaved work is kept.
- Host management from the app (add, edit, test, key login), streamed file and folder transfer with progress and cancel,
  port detection and forwarding, a workspace switcher, and dev containers and WSL over the same agent.

**Git**
- Status, stage, unstage, discard, commit, branches, merge, rebase, tags, remotes, worktrees, stashes, pull, push, fetch and sync.
- A commit graph (Graph, List and Days views) with commit details and a context menu (cherry-pick, revert, reset, branch and tag here).
- A Timeline and local history per file, inline blame, gutter changes, stage or revert a change at the cursor, and git decorations in
  the explorer, tabs and changes list.
- A three-way **merge editor**, and GitHub pull requests (list, check out, open) through `gh` or the public API.

**Language servers and debugger**
- Language servers start on demand per language, with diagnostics, completion, hover, signature help, definition, references,
  rename, code actions, formatting, inlay hints, semantic tokens, code lens, hierarchies, peek, and an Output panel. Defaults exist for
  many languages; a missing server gives one notice with an install hint.
- A **debugger** (Debug Adapter Protocol): breakpoints (conditional, logpoints, hit counts, data), stepping, call stack, variables,
  watches, exceptions and threads, with launch and attach configurations from `.vscode/launch.json` or `.codeyard/launch.jsonc`
  or detected for Cargo, Go and Python. C, C++, Rust and Swift use `lldb-dap`; JavaScript uses js-debug, downloaded on request
  and verified against a pinned checksum. Over SSH the adapter runs on the server.

**Tools without extensions**
- **JSON and YAML schemas.** Validation, hover and suggestions without a language server, from `$schema`, a YAML modeline,
  settings or SchemaStore, cached on disk.
- **Spell check** for prose and code comments with Hunspell dictionaries (English built in, others on request), quick fixes,
  a personal dictionary and a developer word list.
- **`.http` files.** Requests sent with `curl` from the workspace host, with variables and chained responses.
- **Markdown preview**, drawn natively.

**Interface**
- **Turkish.** The whole interface is translated (`ui.language`: `auto`, `en`, `tr`; default follows the system), with Turkish case
  rules for Uppercase, Lowercase and Title Case. Translations are plain `locales/<code>.tsv` files with plural support;
  `docs/translating.md` explains how to add one.
- **Themes.** Built-in dark and light themes with WCAG AA contrast enforced by tests, contrast repair for imported VS Code themes,
  file icon themes, and `workbench.colorCustomizations`.
- **Menus** follow the platform: NSMenu-like on macOS, in-window menu bar on Windows and Linux.
- **Accessibility groundwork.** An AccessKit tree for the window, explorer, editor text and status bar on macOS. VoiceOver itself
  has not been verified (see `docs/accessibility.md`).

**Privacy and security**
- No telemetry and no account; the only network requests are the ones you start. A panic writes a crash report next to the
  state database and sends nothing.
- **Workspace Trust.** In an untrusted folder, project settings cannot set `terminal.shell` or `lsp.servers`, language servers do not
  start, and tasks, tests and the debugger ask first. `$schema` URLs are downloaded only from SchemaStore.
- `.http` requests and schema downloads ask before the first request to each remote host (`http.confirmRequests`).
- Settings, themes and hot-exit backups are written atomically; the Markdown preview opens only `http`, `https` and `mailto` links.

**Distribution**
- macOS (Apple silicon) app bundle and DMG, a standalone tar.gz, and Windows x64 setup EXE and portable ZIP.
  Every release ships `LICENSE-MIT`, `LICENSE-APACHE`, `THIRD_PARTY_NOTICES.md`, `THIRD_PARTY_CRATES.md` and the third-party licence texts.
- Quality gates in CI and in `scripts/check.sh`: rustfmt, clippy, cargo-deny, licence checks, architecture budgets, documentation checks,
  and memory and size budgets (`scripts/memcheck.sh`). `unwrap` and `expect` are denied in the `agent`, `host` and `proto` crates.

### Known limitations
- **No Linux release.** It builds and the app test suite passed once in a container, but there is no installer or CI release job.
- **Windows is unverified on real hardware** for SSH authentication, mixed-DPI displays, IME input and the installer. Windows SSH needs
  a key or ssh-agent and a known host entry, and in-app port forwarding is unavailable there.
- **Not notarized and not signed.** macOS builds are ad-hoc signed (right-click, **Open** on first launch); Windows builds are unsigned.
  `scripts/notarize-macos.sh` has not been run.
- **Apple silicon only** on macOS; there is no Intel build.
- **Screen readers.** Support is groundwork only.
- No extensions, by design.

[Unreleased]: https://github.com/doancan/codeyard/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/doancan/codeyard/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/doancan/codeyard/releases/tag/v0.1.0
