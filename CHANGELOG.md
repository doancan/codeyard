# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Cache CI dependencies; run macOS tests/budgets and Windows tests/standalone EXE checks in parallel, keeping the
  `macos` and `windows` checks dependent on both halves. Install audit/coverage tools from binaries, make manual coverage
  explicit and use Node.js 24 CI actions.

## [0.4.0] - 2026-10-26

### Added

- **A command center in the macOS title bar:** back / forward and a search box with the folder (and SSH host) name in the middle;
  click it to go to a file. The sidebar button on the right is gone: the activity bar already shows and hides the sidebar.
- **Source Control graph actions:** fetch, pull and push buttons (with the commit counts) in the GRAPH header, and a **…** menu
  for the view, scope, *Only My Commits*, *Hide Merge Commits*, search and the branch / tag / sync actions.
- **Terminal line editing on macOS:** ⌘← / ⌘→ go to the start / end of the line, ⌥← / ⌥→ move by word, ⌘⌫ / ⌘⌦ delete to
  the start / end and ⌥⌦ deletes the next word. Full-screen programs such as vim still get the plain keys.
- **Find in the terminal** (⌘F, Ctrl+Shift+F): literal, smart-case search through the output and scrollback with a result
  count, every match tinted and the current one selected.
- Terminal: select all (⌘A), page scrolling (⇧PageUp / ⇧PageDown), jump to top / bottom (⌘Home / ⌘End) and a right-click menu.
- GitHub release signing and Apple notarization for macOS DMG and portable binaries, with credentials stored only in
  Actions Secrets and a temporary runner keychain. Signed dry runs are available without publishing.

### Changed

- **The graph is one line per commit,** and each row is only as wide as the branches beside it: history starts at the left edge
  and side branches step to the right. Views and filters moved from the button row into the **…** menu; active
  filters show as chips that remove themselves on click. The short hash is off by default (`"git.graph.showHash"`).
- The status bar is taller (26 px) with one icon size and even spacing.
- With nothing to commit, the graph starts right under the Changes group and fills the panel (until its height is dragged by hand).
- Build release agents separately so macOS and Windows app builds run in parallel; cache compiled Rust dependencies.
- Upgrade release and benchmark checkout/artifact/cache actions to Node.js 24 releases, pinned by commit.
- Add a Linux-to-Windows MSVC benchmark with shared FXC shaders and native Windows standalone verification.

### Fixed

- **Local branches with a slash** (`feature/x`) were drawn and sorted as remote branches in the graph, and a remote branch was
  shown next to its local twin.
- The graph did not update when a ref moved without HEAD moving: checking out a new branch on the same commit, a fetch, a push,
  a new tag or a branch created in the terminal.
- On Linux and Windows, Ctrl+K in the terminal cleared it instead of reaching the shell (kill to end of line); clear is
  Ctrl+Shift+K there now.
- An empty Changes group no longer shows a "0" badge.

## [0.3.2] - 2026-10-26

### Added

- **Codeyard One**, a dark theme drawn from the colours of the icon: graphite surfaces, a mint accent, keywords and focus ring
  (`"theme": "codeyard-one"`). Every text pair passes WCAG AA.

### Fixed

- **A very long line no longer freezes the window.** Every coloured bracket, diagnostic or IME range split the runs of its line again, so a
  minified file (one line of hundreds of kilobytes) took minutes to lay out. It is linear now.
- **A deeply nested YAML file no longer ends the app.** A few kilobytes of `- - - -` overflowed the stack of the YAML parser that also checks
  every YAML file open in the editor, and of the Data tools.
- **Scanning a long line for colours** (`#fff`, `rgb(…)`) took time with the square of the line; it is linear now.
- `editorconfig`: the glob length limit now also holds when the function is called directly.

### Changed

- **Releases build in about half the time.** Dependencies come from a build cache, the SSH agents are built in their own jobs (the Linux
  ones without zig) and the macOS and Windows builds run side by side. The third-party actions of the release workflow are pinned to commit SHAs.

## [0.3.1] - 2026-10-26

### Added

- **Right click on a changed file in Source Control** (pull request #3) opens a menu: *Copy Path*, *Copy Relative Path* and *Add to .gitignore*.

### Fixed

- **Crash on macOS when answering the update dialog.** Every click or key in the *Software Update* window (*Check Daily*, *Only When I Ask*,
  *Install and Restart*, *Later*, the red close button) asked the editor window to run it in a way that updated the workbench twice, and
  the app aborted. The dialog's window now hands the answer over without re-entering the workbench. New tests drive the dialog through
  its own window, which none did before.
- **Menu bar and Dock menu rebuilt in the middle of a window update.** Changing the interface language (or the recent list) from inside
  an update rebuilt the menus at once, and macOS validates every item during that, which could update the same window again. The rebuild now
  waits for the end of the current update.
- **Windows** (pull request #1): find the SSH configuration and VS Code settings without the Unix `HOME` variable; keep Windows drive and
  network-share paths in workspace files and file URLs, and open terminal links through the system browser; move files to the native
  Recycle Bin so Explorer can restore them; launch without a separate console window and embed the application icon in the EXE.

- **Workspace files no longer probe network shares.** A `.code-workspace` file from a cloned repository could name `\\server\share` folders, and on Windows
  looking at them connects to that server with the user's credentials. Such folders are now taken only when the workspace file is on that same server.

### Changed

- **No file-access dialogs at start on macOS.** Codeyard no longer touches Desktop, Documents and Downloads (and so iCloud Drive) on the first
  start of each version; macOS asks when you open a folder there, as with other editors. `CODEYARD_SKIP_DISK_PROMPT` is gone.
- **A shorter first-run question about update checks.** Two short sentences instead of two paragraphs; it still says that github.com is
  contacted, that GitHub sees your IP address and nothing about your files, and where to change it.
- **Windows builds embed the Linux x64 and ARM64 SSH agents** (pull request #1), so a copied EXE connects without an `agents` folder. Source
  builds and packaging prepare the agents automatically, and standalone checks verify the EXE.

## [0.3.0] - 2026-10-26

### Added

- **Structural search and replace.** *Search: Structural Search…* (⌥⇧⌘F) and *Search: Structural Replace…* find code by its shape, not its text:
  a pattern is code in the file's language where `$X` matches one node and `$$$X` any run of siblings (`$R.unwrap_or($X.clone())`
  becomes `$R.unwrap_or_else(|| $X.clone())`). It works in 15 languages with the tree-sitter grammars Codeyard already ships, reads
  only the files that can match, shows a pattern that does not parse inline, and replaces through the same flow as *Replace in Files*
  (files that would no longer parse are named first). Setting `search.structural.maxFiles`.
- **Compare folders.** *Compare: Folders…* (and the tree menu) compares two folders, each on this computer or on the connected server, by
  size and content hash: only left, only right, changed, identical. A changed file opens in the diff view; files and folders can be
  copied either way or deleted on a side you mark writable, always after a confirmation and never over a file that changed since the
  scan. Over SSH only the file list and hashes travel. This raises the agent protocol to version 10, so servers get the new agent on the
  next connect. Settings `compare.folders.ignore` and `compare.folders.maxFiles`.
- **Data tools.** *Data: Convert To…* turns JSON, YAML, TOML and `.env` into one another (and says what a format cannot hold),
  *Data: Query…* runs a jq-style query live, *Data: Copy Path at Cursor* copies `.servers[2].host` (also shown in the status bar), and
  *Data: Decode Selection…* reads a JWT or a Unix timestamp. Everything is local. Settings `data.indent` and `data.sortKeys`.

- **Git sync button.** Source Control shows **Sync Changes ↓N ↑M** (or **Publish Branch** when the branch has no upstream); the status bar
  item has the same counts, a spinner while it runs and a tooltip with the upstream. One sync runs at a time, a pull that conflicts says
  so, and a dirty tree is told to commit or stash. New commands: *Pull (Rebase)*, *Pull from…*, *Push to…*, *Fetch (All Remotes)*,
  *Commit Signed Off*, *Commit (No Verify)*, *Commit & Push*, *Commit & Sync*. New settings `git.autofetch`, `git.autofetchPeriod`,
  `git.confirmSync`, `git.rebaseWhenSync`, `git.pruneOnFetch`, `git.pullTags`, `git.fetchOnPull`, `git.postCommitCommand` and
  `git.enableSmartCommit`.
- **More Git.** Annotated tags (a message after the name), *Delete Tag*, *Delete Remote Tag*, *Push Tag(s)*; the branch picker shows
  ahead/behind, the last commit and its age, sorted by recency, with *Checkout (Detached)*, *Create Branch From*, *Delete Remote Branch*
  and *Set Upstream Branch*; *Stash Staged Changes*, *Stash with Name…*, *Show Stash…*; *Open Changes*, *Open File (HEAD)*, *Compare with
  Branch…* and *Compare with Commit…* for the active file; *Add to .gitignore* in the tree menu; *Rename Remote*, *Edit Remote URL*,
  *Remove Worktree* and *Prune Worktrees*.

### Security

- Pull request checkout took the remote name from the repository's `.git/config` and handed it to `git fetch` as an option, which could
  run a program named there. Remote names that start with `-` are refused and `--` ends the options.
- `.http` requests no longer follow redirects by themselves: a 307 to `127.0.0.1` or the metadata address re-sent the body and the custom
  headers. The redirect is shown with its target instead.
- A folder trusted on this computer was also trusted when an SSH window opened the same path on a server; trust now includes the host.
- A language server can no longer make Codeyard write to files outside the open folders (`workspace/applyEdit`).
- Developer flags that run palette commands (`--do`) are ignored unless Codeyard is a debug build or `CODEYARD_DEV` is set.
- HTTP responses kept for the response tab, crash reports and the restart manifest are private to your account, and responses are
  deleted when the tab closes. The GitHub token no longer appears in the process list.
- A restricted (untrusted) folder can no longer make Codeyard read a network share through `$schema` on Windows.
- A manual run of the release workflow signed with the real update key; only tag builds see it now.
- A language server that never answers `initialize` is stopped instead of left running; its messages and a debuggee's output are clipped
  and bounded so one chatty process cannot freeze the window.

### Changed

- `terminal.allowClipboardWrite` (default off): a program in the terminal can no longer replace your clipboard with an OSC 52 sequence.

### Fixed

- Unsaved work: quitting or switching folders now asks when a dirty file cannot be backed up (a file outside the folder, a full disk, no
  state database); a forced save over a file that changed on disk keeps the overwritten version in local history, and a second Cmd+S
  within half a second no longer forces it; Save As keeps the old tab when you kept typing during the write.
- A state database that cannot be opened is reported, and a damaged one is set aside (`state.db.corrupt-<time>`) and replaced.
- Over SSH, a large write no longer freezes the window while it uploads: the connection has its own writer thread. A write that fails
  in the agent no longer leaves a hard-linked file empty, and a target that grew past the size limit is refused instead of read whole.
- The `.http` response tab no longer closes the request file itself when you send twice.

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

[Unreleased]: https://github.com/doancan/codeyard/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/doancan/codeyard/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/doancan/codeyard/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/doancan/codeyard/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/doancan/codeyard/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/doancan/codeyard/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/doancan/codeyard/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/doancan/codeyard/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/doancan/codeyard/releases/tag/v0.1.0
