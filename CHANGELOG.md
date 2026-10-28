# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Paste clipboard images into programs such as Codex in an SSH terminal: the image is transferred to a private temporary
  file on the remote host and its path is pasted into the terminal.
- Local SSH port forwarding on Windows using a dedicated OpenSSH process for each forward.
- Installable declarative extensions: themes, snippets, keymap presets and key bindings, languages, JSON and YAML schemas,
  settings and language servers. The Extensions view installs a `.cyx` package or a folder after a consent sheet that
  lists contributions and permissions in plain words (High-risk sets in red, with *Cancel* focused), and disables,
  uninstalls, rolls back or verifies an extension without a restart. `--no-extensions` loads none and says so in the
  status bar.
- Four built-in dark themes: Catppuccin Mocha, Tokyo Night, Gruvbox Dark and GitHub Dark, with complete colour roles
  and WCAG AA contrast across all 103 text and UI pairs before runtime adjustment.
- Accessible Source Control change groups, commit message and commit state, file diffs, Stage/Unstage actions and live
  operation results. Hidden Explorer trees no longer appear in the accessibility tree, and stale file actions resolve paths again.
- Cargo target discovery for automatic Rust debugging: virtual workspaces, multiple binaries, examples, required features
  and library-only test harnesses. Programs come from Cargo's artifact messages, including custom target directories.
- *Rust: Open Cargo.toml* falls back to the nearest manifest inside the file's workspace when no language server is attached.
- Source Control lists Git worktrees with their branch, path and current checkout, plus add/refresh controls and
  keyboard-accessible same-window navigation on local and SSH hosts. Detached, locked, stale and bare entries are identified;
  worktree paths with spaces, Unicode, quotes or line breaks are preserved by NUL-delimited Git output.

- macOS accessibility landmark bounds for the window, primary Explorer viewport, focused text-editor pane
  and status bar, with display-scale conversion and updates when resizing, switching split groups or hiding panels.

- Full-width Container Overview table with in-page search, live SSH resource data, health filtering,
  selectable columns, stable refresh ordering and scrolling, detail navigation, logs and container actions.

- Optional Web Preview module in Settings: a built-in local static server, browser preview from HTML tabs,
  live page/CSS refresh, status-bar controls and Output logging. Local, SSH and container files share the
  same agent-backed path; no external runtime or remote listening port is required. Disabling the module
  or leaving the workspace stops its server.
- Web Preview settings for a fixed or automatic port, IPv4/IPv6 listen address, live reload toggle and interval,
  and a custom served folder. Options appear under Modules, belong to user settings, and restart an active preview when changed.
  *Web Preview: Copy URL* copies the authenticated launch link for use in another browser or device.
- File icon theme "File Glyphs" (`workbench.iconTheme: "glyphs"`, now the default): colored file glyphs and no folder icons. A
  file's glyph sits in the column of a folder's arrow, so every name in a level lines up.
- The macOS Dock menu lists the open windows at the top, front one first, and follows as windows come forward, open, close or
  change title; choosing one brings it to the front. Recent projects follow, newest first, without those already open.
- `terminal.lineHeight` sets the terminal's row height as a multiple of the font size (1–2, default 1.2).
- Language servers take their own configuration from `lsp.servers`: `settings` (what the server reads as its configuration,
  dotted keys allowed) and `initializationOptions`, with or without `command`. Rust developers can now turn on Clippy on save
  (`"rust-analyzer.check.command": "clippy"`) and analyse code behind Cargo features. Changing `settings` reaches a running
  server without a restart; it works the same in SSH folders.
- Problems explain themselves: the places a language server attaches to a problem ("value moved here", "borrow later used
  here") show under it in the hover and in Problems, and a click opens them. The problem's code comes first (`E0382: …`), and
  *Problems: Copy Full Diagnostic* copies the compiler's complete message, as a terminal shows it.
- Rust commands: *Rust: Expand Macro Recursively* shows a macro's expansion in a scratch tab, *Rust: Locate Parent Module*
  jumps to the `mod` line that includes the file, and *Rust: Open Cargo.toml* opens the crate's manifest.
- Screen readers on macOS see the editor tabs as a tab list (name, modified, the tab in front) and can bring a tab to the
  front. Notices are spoken when they appear: errors, including a file that could not be opened, interrupt speech. Before,
  errors were not exposed at all.
- `ACCESSIBILITY.md` states what works with a screen reader, the keyboard, zoom and high-contrast themes, the known gaps, and
  how to report a barrier with the new *Accessibility barrier* issue form.
- Screen readers on Windows: NVDA, JAWS and Narrator get the same accessibility tree as VoiceOver, through UI Automation.
- Screen readers reach the command palette and every other picker (query, list, the selected row as it changes), the find bar
  (fields, the match count as it changes, options with their state, buttons), suggestions while typing, the terminal's
  screen and every folder of a multi-folder workspace.
- `workbench.reduceMotion` leaves out animations (blinking cursor, spinners, fading panels, the sliding progress bar); `auto`
  follows the system's Reduce motion setting on macOS and Windows. The *Auto* theme switches to the high-contrast pair while
  the system asks for more contrast.
- Choosing a theme or changing `workbench.colorCustomizations` says so when the colours in use fall below WCAG AA contrast,
  with the first failing pair.
- The debugger shows Rust values as values: lldb-dap starts with the formatters of the toolchain that builds the program, so
  `String`, `Vec`, `HashMap` and `Option` read as text and elements, and with `rust-src` installed stepping into the
  standard library opens its source. Locally and over SSH (the server's toolchain); the Debug Console says when they cannot
  be loaded.

### Changed

- The About page groups application, platform and client protocol details, and shows the remote agent protocol when connected.
- Releases are gated on performance instead of app size: `scripts/perf` compares startup, typing latency and memory with
  the previous release and blocks regressions beyond their budget. The app binary size limit is removed; the SSH agent keeps
  its limit.
- Sync no longer asks before it pulls or pushes: `git.confirmSync` now defaults to `false` (turn it on in your own settings to
  get the question back). The status bar keeps the ↓/↑ counts visible at every width, marks a branch with uncommitted changes
  with `*` and says "Publish Branch" for an unpublished branch.
- The terminal keeps 14 px of space at the sides and 8 px at the top and bottom instead of running into the panel's edges, and its
  rows are 1.2 × the font size instead of the editor's looser 1.5 ×.
- File tree rows are a little taller and roomier (24 px rows, wider side padding and icon gap).

### Fixed

- Terminal Space sends a space, and Ctrl+C copies a selection while still interrupting the program when nothing is selected.
- SSH connections detect an incompatible bundled agent before attaching to its daemon and explain how to rebuild or reinstall it.
- Web Preview reads files through directory handles without following symlinks or reparse points,
  preventing pathname replacement races. Each file now needs one agent request, reducing SSH round trips;
  directory and session redirects preserve URL query parameters.

- Opening or selecting a file restores a maximized terminal panel to its previous size so the editor is visible.
- Picker inputs support Select All, Copy, Cut and Paste, including remote folder paths and SSH password paste.
- Switching SSH hosts resets the workspace even when both servers use the same folder path, and preserves the
  previous server's saved tabs. Disconnecting clears remote folders, Git state and project settings without overwriting its session.
- The empty Explorer's workspace link stays on one line in narrow sidebars.
- Run and Debug shows an automatic session's real name; without a launch file its selector says "Auto-detect project"
  instead of claiming there are no configurations.
- Debugging saves workspace buffers before building or starting, honors format-on-save, and cancels on save errors,
  disk conflicts, Stop or a workspace switch instead of debugging an older on-disk program.
- Stop invalidates pending target discovery and test-build replies; late adapter starts, pre-launch errors and stopped
  events cannot revive a cancelled debug session.
- Right Arrow stays on an expanded empty Explorer folder instead of selecting a sibling.
- Rust commands distinguish a non-Rust file, disabled language features, an untrusted workspace and an unavailable server.
  Old Rust replies and test-discovery/build replies are discarded after a workspace change.
- Rust test discovery recognizes parameterized async test attributes and raw identifiers. File/cursor test commands use the
  actual nearest manifest instead of guessing a package name from a directory.
- Debug Test at Cursor refuses unrelated helper functions; failed or interrupted test builds cannot launch partial artifacts.
  Build diagnostics reach Output and Problems, and quoted manifest/target/feature arguments remain literal argv.
- Rust workspace test runs continue after a target fails. Duplicate test names cannot overwrite a failure with a pass,
  and unreported tests cannot keep an earlier green result after a build or runner failure.
- Memory checks measure and stop their own disposable agent daemon instead of looking up an unrelated running daemon.
- Windows CI also runs Rust Cargo/debug, product and accessibility regressions; the scripted adapter fixture launches
  through Python explicitly instead of relying on executable script associations.
- Rust build errors and test panics normalize Windows drive/UNC paths to the editor's path form, restoring matching
  Problems entries and squiggles. Spaces and drive letters in panic locations are preserved; POSIX backslashes remain literal.

- The workspace-symbol test ran with language servers turned off, so it could never pass; it now starts rust-analyzer.

### Changed

- Port mappings and mounted storage use responsive inspection cards with copyable addresses and paths.
  Log entries place timestamp and severity above full-width messages, with a full-width search and console footer.

- Container details promote the selected name and file/terminal actions into the header. Responsive CPU/memory
  cards show current readings above charts, disk/network activity shares the available width, and log search has a visible field frame.

- Container Overview fills the available width when resizing or hiding columns. Name and Image expand while
  metrics stay aligned with larger table text; header actions and search/filters wrap in narrow windows. Sorting moves to the
  Name/CPU/Memory column headers, and Clear filters appears only for an active query or filter.
- The Container Overview header combines connection, update state and fleet counts, with grouped filters
  and a framed search field. Redundant initial navigation and duplicate count rows are removed.

## [0.5.0] - 2026-10-27

### Added

- A status-bar resource popover shows local CPU, memory, Codeyard memory, free disk space and local project size. It fades
  open on click, closes when the pointer leaves, and can be pinned. Sampling stops when closed; project scans are bounded.
- Changed-file context menus now offer Open Changes, Open File, Stage, Unstage and confirmed Discard according to the selected
  staged/unstaged row. Conflict resolution keeps the existing marker checks.
- Git discard confirmations put Cancel first and explicitly bind it as the cancel action in every language.
- `terminal.scrollSensitivity` adjusts wheel/trackpad speed (default 2×); small trackpad movements accumulate instead of disappearing.

- Panel and sidebar section resize handles gently fade their accent on hover and exit, while staying clear during a drag.
- Container logs use a dedicated console with numbered rows, separate UTC timestamps, severity labels, error highlighting,
  literal search highlights, an error/warning filter, copy-visible output and line-wrap/follow controls. Scrolling upward
  pauses following while snapshots continue; ANSI control sequences are removed from display and clipboard text.

- Container browsing now groups Compose projects, searches names/images/projects and sorts CPU or memory within groups.
  Compact navigation opens a selected-container workspace with bounded 1/5/15-minute CPU/memory history, counter-safe
  disk/network rates, automatically refreshed log snapshots, inspected mounts/health and direct file/terminal entry.

- SSH workspaces automatically discover containers through the existing agent. The Containers sidebar and resource-monitor tab
  show live CPU, memory/limit, disk size, disk I/O, network I/O and process counts, with refresh/pause controls, inline errors
  and direct connect/log actions. Resource snapshots refresh while visible; disk size is sampled at most every 30 seconds.

- An optional container module (`modules.containers.enabled`, off by default, user settings only): Docker context selection,
  container status and logs, starting stopped containers, and Dev Container creation/rebuild through the separately installed
  reference CLI, with cancellation, bounded output and workspace-change guards. Apple Container, WSL Containers and Windows
  guest requirements and native adapter limitations are documented separately.

- Markdown files have a **Read** button to open the native reading view and a **Code** button to return to the source without
  losing edits. Code blocks support syntax highlighting, horizontal scrolling and **Copy code**; heading links navigate within
  the document, relative file links open in Codeyard, and relative images load from local folders or SSH hosts.


- **Remote Explorer view** in the activity bar: SSH hosts with their recent folders and connection status, running
  containers (and WSL distributions on Windows), and the forwarded ports of the current connection. Connect or open a
  folder in this window or a new one, forget a folder, and edit, test, set up key login for or remove a host from the
  row. *View: Remote Explorer* opens it.
- **Ports table:** the PORTS panel lists Port, Label, Forwarded Address, Running Process and Origin, with buttons to open,
  copy and stop, and a menu to set a label, change the local port, choose HTTP, HTTPS or TCP, or retry. `3000:8080` in the
  field forwards server port 3000 to local port 8080.
- Forwarded ports are saved per project and forwarded again when the project is opened over SSH, with their label,
  protocol and chosen local port.
- `ports.localBind` (`loopback` or `all`; only from your own settings) and `ports.rules` (label, protocol and `notify`,
  `silent`, `openBrowser` or `ignore` per port or range).
- The ssh config's `Include` lines are followed when listing hosts.
- **Run and Debug view** in the activity bar: a configuration selector with a start button, and folding, resizable
  sections Variables, Watch, Call Stack (threads as a tree, one row per session when several run) and Breakpoints.
  While a session runs, a toolbar floats over the editors; the bottom panel keeps the Debug Console.
- **Breakpoints** are saved per project and come back after a restart. They can be disabled one by one or all together,
  and deactivated without being lost; the adapter's answer marks each one, and the gutter shows a disabled breakpoint as
  a muted ring and one the debugger could not bind as a red ring. Function breakpoints are new.
- Variables: *Set Value*, *Copy Value*, *Copy as Expression* and *Add to Watch*; values with many elements load 100 at a
  time. Call Stack loads 20 frames at a time and offers *Restart Frame* and *Copy Call Stack*. The Debug Console keeps a
  history (↑ / ↓) and completes with Tab.
- **Run Without Debugging** (⌃F5), **Run to Cursor**, launch **compounds** (with `stopAll`), `${input:…}` questions
  (`promptString`, `pickString`), `postDebugTask`, **Add Configuration…** templates, programs that ask to run in a
  terminal (`"console": "integratedTerminal"`), several sessions at once, and a **Run** menu. F6 pauses.
- **Terminal shell integration** for zsh, bash and fish: a dot beside each command (green succeeded, red failed),
  jumping between commands (⌘↑ / ⌘↓), *Copy Last Command Output*, and splits that open where the shell is.
  `terminal.shellIntegration` turns it off. Links that programs mark (OSC 8) open their real target.
- **Terminal profiles** (`terminal.profiles.macos` / `.linux` / `.windows`, `terminal.defaultProfile`) with *New Terminal with
  Profile…* (also listing the machine's shells) and *Select Default Profile…*; terminals can be renamed and coloured, and
  *Kill All Terminals* closes them all.
- **Testing view** in the activity bar: the tests of the open folder (Rust, Go, Python and JavaScript), found in the
  background and grouped by file. Run a test, a file, all tests or the failed ones with the project's own runner, or stop
  the run; output streams into a *Test Results* terminal, and each test shows its outcome, time and first failure line,
  which opens where it failed. Rust, Go and Python tests can be debugged from their row or with *Test: Debug Test at
  Cursor*. A filter and a failed-only toggle narrow the tree; the activity icon counts failures.

### Changed

- Markdown resource loads keep their own identity: an old read cannot overwrite a newly added image with the same URL.
- Readiness and platform documentation now match the blocking Linux checks and verified macOS release notarization.

- **Platform-standard code font size:** the editor and terminal default to 12 px on macOS and 14 px on Windows and Linux
  (was 13 px everywhere). A size written in your settings file is kept. The settings template now leaves the font keys
  commented out, so a settings file shared between computers does not pin one platform's size.
- The status bar is 28 px high.
- Bottom-panel tabs use readable 13 px title-case labels, a subtle rounded selection background and hover fill instead of an underline.
- The initial update-check prompt uses a short explanation of the daily option and where to change it.
- Closing a terminal asks first while a program runs in it, and names the program (`terminal.confirmOnClose`).
- `terminal.scrollback` is capped at 50,000 lines, and a change applies to open terminals.
- Every text size in the interface comes from the type scale (new *badge* and *display* steps replace the remaining fixed
  sizes); a test keeps new code on the scale.

### Security

- Upgrade the vendored UI's Taffy pin to 0.9.2 to remove vulnerable `grid 0.18.0` (GHSA-38c5-483c-4qqp).
  The dependency policy now rejects the affected grid versions independently of RustSec advisory coverage.
- Markdown images now use the same 20 MB and 50-million-pixel checks as image tabs, before decoding. SVG canvas sizes
  use the renderer's XML and unit rules, so comments and attribute spacing cannot bypass the limit.

- **Git is paused in untrusted repositories whose own settings run programs.** Opening a folder ran `git status`, and
  `git status` runs a repository's clean filter on every file it sees as changed, which is every file of a checkout
  copied from an archive. In a folder you have not trusted, a repository whose `.git/config` names a program (a filter,
  diff or merge driver, an ssh, askpass or credential command, a hooks folder) now gets no git at all until it is
  trusted, and a notice names the settings.
- Debugging a test from the Testing view built it (running its build script) before asking for trust, and running
  tests asked about the active file's folder rather than the one they run in. Both now ask for the folder the command
  runs in.
- A debug adapter, or whatever answered on its port, could run a program in a terminal by asking (`runInTerminal`)
  even when no terminal was offered; such a request is now refused.
- A `.http` file could hide that a request sends environment variables by going through a file variable, and a body
  file reached through a symbolic link in the folder was uploaded without a question. Both are now confirmed.
- A project's settings can no longer turn on clipboard writes from the terminal or turn off the paste checks
  (`terminal.allowClipboardWrite`, `terminal.sanitizePaste`, `terminal.warnMultilinePaste`).
- Commands typed into fish could be broken out of by a test or file name containing a backslash and a quote; quoting
  now reads the same in fish as in POSIX shells.
- Tasks and launch configurations read for one folder are no longer run under another folder's trust when the active
  file changes while they load.

### Fixed

- Licence metadata encoding tests run with the macOS system Python without relying on private subprocess APIs.
- Outline and Timeline resize from the line above their headers, using their actual displayed height. The 28 px status
  bar has larger icons and keeps resource controls visible when long branch names or messages meet a narrow window.
- Long Git/context menus keep full-height rows and scroll the keyboard selection into view.
- Busy status bars compact sync, conflict and save indicators and yield secondary editor details before controls overflow.
  Long branch names keep their full tooltip, and problem-count tooltips retain the full error and warning totals.
- Find/Replace uses the whole replacement field as an input target. Its controls wrap within narrow editor panes so
  Close and Replace All remain reachable.
- Inline Git blame and language-server line-end hints use measured text widths and yield to code in narrow editor groups.
- Source Control file and commit hover cards stay hidden while a context menu is open, so they cannot cover its actions.
- Settings edits find inline root keys, escaped keys and nested JSONC values without inserting a duplicate or changing
  nested settings. Theme changes now apply to compact settings files too.

- SVG previews preserve their red/blue channels and translucent colors; long Markdown paragraphs wrap inside the document column.
- Text links follow only left clicks, accept a press/release arriving in one frame, and discard cancelled presses.
- Prevented workbench focus handling from swallowing native Windows title-bar drag, double-click and window-control events.

- Removed the redundant container monitor button; selecting a container opens its details. Windows now uses a themed title bar with native drag, minimize, maximize/restore and close control areas.

- Container sidebar filters now show explicit states, result counts and reset; background refresh keeps resource sort order stable and avoids loading-message layout shifts.

- Refined container resource history, separate rate and total readings, structured port bindings and mount access details with copy and refresh actions.

- Container resources now show only the container selected in the sidebar. Search, filtering and sorting stay in the sidebar.
  Opening container files retains the source SSH connection/folder and exposes a persistent return action that restores
  the workspace and selected-container details without a new SSH login; unsaved-file guards apply.

- Container actions in an SSH workspace execute Docker on the connected server; listing, start, logs and agent bootstrap
  retain the SSH host and Docker context, with no local Docker installation required.

- Container actions no longer assume Docker is installed: choose Apple Container, WSL Containers, Podman or Docker as appropriate
  for the host platform. Missing engines and the Dev Container CLI show setup instructions before execution, including restored
  connections; GUI launches also find common CLI installation paths. Native Apple/WSLc/Podman adapters support list/start/attach/logs;
  WSLc/Podman Dev Containers use the reference CLI, with unsupported creation modes explained before setup.

- Enabling the container module now adds a Containers button and sidebar to the left activity bar; disabling it hides both.

- The container module toggle is visible at the top of Commonly Used settings and in a dedicated Modules category.

- Markdown heading links remain distinct when repeated headings collide with numbered titles (for example, "Setup", "Setup-1", "Setup").


- Memory benchmarks measure normal editor scenes after choosing manual update checks and cover the first-run question
  separately, so the question's extra native-window buffers do not inflate every editor measurement. Existing budgets stay unchanged.
- Generic editor/workbench test fixtures no longer start installed language servers; language-server integration tests opt in explicitly.

- Release signing and verification detect `pkeyutl -rawin` support instead of rejecting OpenSSL 4 by its version string.

- The terminal ignored `terminal.fontSize` and zoom: it now uses the setting, with rows sized to match.
- **Pasting into the terminal is safer:** bracketed-paste markers hidden in pasted text can no longer end the paste early
  and run what follows; other control characters are removed when the shell does not use bracketed paste
  (`terminal.sanitizePaste`), and a paste of several lines asks first (`terminal.warnMultilinePaste`).
- A split terminal opened in the folder the first terminal started in, not where its shell was.
- Stopping a process the editor started (a test run, a debug adapter) now also stops the programs it started.
- **Port forwarding is more reliable:** a double click no longer forwards a port twice; forwards restored after a
  reconnect report failures on their row (with *Retry*) instead of looking active; switching or losing the connection
  cancels forwards instead of leaking their local ports; after a drop, forwards come back on the same local port.
- A forward to a port held by another program on this computer's IPv4 loopback looked active while connections went to
  that program; busy ports are now detected on both loopback addresses.
- Forwards connect to the exact address a server port listens on (`127.0.0.1`, `::1` or another address), so a service
  bound to one of them is reached; the Ports panel names the program listening on each port, and a failed scan keeps
  the last list.
- Programs named without a path (git, curl, language servers, debug adapters) failed to start for users whose login
  shell is fish; they now start through fish, and through `/bin/sh` for other non-POSIX shells.
- **Two terminal crashes**: scrolled back while a program with a scroll region (vim, less, tmux) printed new lines, and
  *Copy Last Command Output* after a program cleared the history.
- A language server's rename edit of an unexpected shape, or a theme file that is not an object, no longer crashes the
  app.
- Opening an image that announces enormous dimensions (a small PNG or SVG can ask for gigabytes once decoded) no longer
  exhausts memory: images over 50 megapixels are not decoded, and the preview says how large they are.
- A language server or debug adapter printing endless output without message headers made the app spend a minute of
  CPU per 9 MB; that output is now read in linear time.
- Finding the tests of a file with thousands of tests took tens of seconds; it takes milliseconds. Rescanning a long
  task terminal for problems is about five times faster.

## [0.4.1] - 2026-10-26

### Changed

- The title bar is 35 pt high so the window buttons sit in its middle; the command center is a slimmer box drawn on the
  title bar with a faint outline and the folder name on its left, centred together with back / forward.

### Fixed

- A port-forwarding test failed when another program opened a port on the machine during the run.

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

[Unreleased]: https://github.com/doancan/codeyard/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/doancan/codeyard/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/doancan/codeyard/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/doancan/codeyard/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/doancan/codeyard/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/doancan/codeyard/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/doancan/codeyard/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/doancan/codeyard/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/doancan/codeyard/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/doancan/codeyard/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/doancan/codeyard/releases/tag/v0.1.0
