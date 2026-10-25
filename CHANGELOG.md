# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.2] - 2026-10-26

### Changed

- **Update window (macOS).** The update dialog is its own small window, like the system's software updates: app icon, title, release notes
  and one row of buttons (*Skip This Version*, *Remind Me Later*, *Install Update*). Progress, restart and errors show in the same window,
  Esc and the red button mean *Later*, and the colours follow the theme (the brand's mint green on dark backgrounds). Windows and Linux
  keep the dialog inside the window.
- **File access prompts.** The start-up dialog about Full Disk Access is gone. The first start of each version touches Desktop,
  Documents and Downloads, so macOS asks in its own dialogs and lists Codeyard under Files & Folders, and Codeyard tries once to get itself
  listed under Full Disk Access too. *Help: Grant Full Disk Access…* stays, and says so when the access is already there.
- A window with a folder open (local or over SSH) keeps the editor area quiet instead of showing the welcome page, as VS Code does; an empty window still opens on it.

### Fixed

- Edits whose new text contains line breaks (`\r\n`, or a stray `\r` in the file) could leave a cursor past the end of the text, and
  the next arrow key, *Toggle Comment* or *Delete Lines* panicked.
- *Replace in Files* and workspace edits (rename) rewrote the line endings of closed files that mix `\n` and `\r\n`; untouched lines
  now keep their endings.
- *Run Test at Cursor* and task variables such as `${file}` put names into the shell command unquoted, so a folder or file name with
  `$(…)` or `;` ran as a command.
- Pinning a folder whose name contains a line break pinned somebody else's folder as well.

## [0.2.1] - 2026-10-25

### Security

- **Git branch names.** A repository can hold a branch called `--exec=<command>` (a server can name its default branch so), and
  *Rebase onto…* passed the name to `git rebase` as an option, which ran the command. Branch, tag and remote names that start with
  `-` are now refused before any git command, and not offered in the pickers. *Merge* and *Checkout* had the same flaw.
- **Folder download.** A server could send more than the folder you asked for, and extraction wrote it next to that folder in the
  directory you picked (`.zshrc`, `.git/hooks`…). The stream is now extracted into a scratch folder, anything but the requested
  folder is refused, and only then is it moved into place.
- **SSH host names.** A host name starting with `-` (a pasted name, a shared `projects.jsonc`, a `Host` line) was read by ssh as an option,
  and `-oProxyCommand=…` runs a program. Such names are refused, and `--` now comes before the host on every ssh command line.
- **Local `$schema` paths.** `"$schema": "/dev/zero"` in any JSON or YAML file made Codeyard read without end until memory ran out.
  A local schema must now be a regular file of at most 8 MB.

### Fixed

- Opening a JSON file crashed the app when a non-ASCII character (any `ş`, `ğ`, `ü`…) sat across byte 4096 of the file; with
  *Restore Session* it crashed again on every start.
- *Format Document* on deeply nested JSON (tens of thousands of `[`) built gigabytes of output; nesting beyond 512 levels is refused.

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

[Unreleased]: https://github.com/doancan/codeyard/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/doancan/codeyard/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/doancan/codeyard/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/doancan/codeyard/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/doancan/codeyard/releases/tag/v0.1.0
