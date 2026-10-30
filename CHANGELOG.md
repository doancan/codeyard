# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Windows: terminals of a local folder are taken back after Codeyard quits, crashes or is killed, as on macOS and
  Linux. They run in the terminal daemon, which listens on a named pipe only the current user can open.
- `cyx dev` hot-reloads code extensions: saving a source file rebuilds `extension.wasm` (cargo or the TypeScript
  pipeline), and the dev window stops the running build, compiles the new one with Cranelift's incremental cache,
  activates it with the reason `reload` and draws its open views again; calls made meanwhile wait for it. A failed
  build leaves the last good one running. `ctx.state` (Rust and TypeScript SDKs) keeps in-memory values across the
  reload, through the reserved storage key `codeyard.reload-state`; the mock hosts have `reload`. On an M1 Max a reload
  takes 30 / 33 ms (p50 / p95) for a 0.2 MB Rust component, 168 / 180 ms for a 1.9 MB one and 616 / 655 ms
  for a TypeScript one.
- `cyx check --fix` adds the missing permissions that the manifest checks and the component's import check report
  (`CY0201`) to `codeyard-extension.toml`, keeping its comments and layout, prints the lines it changed and lists the
  fixes that need the author (placeholders, which sign-in provider, `why`).
- `cyx publish` no longer refuses code extensions itself: the registry marks them as code in its index from the
  package. A registry that takes data-only extensions (the public one, for now) gets a clear message instead of its
  raw refusal.
- Example extensions: `trim-formatter` (a formatter), `todo-actions` (code actions), `crate-version` and
  `crate-version-ts` (`fetch`), and `branch-status` (a program, with `run`), built, unit-tested and run with
  `cyx test` in CI.
- `scripts/check-five-minutes.sh` scripts the "a command in under 5 minutes" goal: `cyx new command`, its unit tests
  and `cyx test` from an empty folder, Rust and TypeScript, each step timed; CI's Linux job runs it.
- An editor tab dragged out of the window and released outside it becomes a window of its own at that place, showing
  the same document; released over another Codeyard window, the file opens there instead and its tab here closes (a
  file with unsaved changes or on another machine stays, with a message).
- A code extension's side bar view can move into a window of its own ("View: Move View into New Window") and to the
  secondary side bar. It stays the same view: the extension's updates reach the window and what you do there goes to
  the extension. The window is restored with the folder while the extension is installed, closes when the extension
  is removed or turned off, and gives the view back to the side bar when the extension crashes.
- Terminals in editor groups and in windows of their own, and the terminals of SSH folders, are taken back after a
  restart the same way as a local folder's panel terminals: the same shells, in their places, with "Reconnected to
  terminal session". SSH folders resume their session on the server's agent, which already kept the shells.
- Setting `terminal.persistentSessionTimeout`: how many minutes terminals keep running for the next start after
  Codeyard quits, crashes or is killed (default 5, at most 1440; 0: they end with the app). The agent protocol is
  version 17.
- "Break on Value Change" in a variable's right-click menu sets a data breakpoint on it (when the debugger supports
  them). Data breakpoints are listed in the Breakpoints section, where each can be turned off or removed by mouse,
  keyboard (Enter, Shift+F10) or screen reader; they last for their debug session.
- Screen readers get the painted bounds of the rows of the Settings and Keyboard Shortcuts pages and of menu items
  (none for a row scrolled out of view).
- On macOS and Linux, the panel's terminals of a local folder keep running when Codeyard quits, crashes or is killed
  (with `window.restoreSession`, the default). Opening the folder again within 5 minutes brings the same shells back,
  with their output from meanwhile, and says "Reconnected to terminal session". A terminal that cannot be taken back is
  a new shell as before, and nothing is ever typed into it. Terminals no window takes back are ended after 5 minutes.
  Closing the window or the folder, or opening another one, still ends them at once. Terminals another Codeyard
  instance has open stay with it: the window starts new shells and says so. The agent protocol is version 16.
- The lists of the Run and Debug view, the Testing view, the Remote Explorer and the Ports panel work with the keyboard:
  Tab reaches them, Up/Down/Home/End move a cursor row shown in the focus colour, and Enter does what a click on the
  row does. Shift+F10 (and the Menu key) opens the focused row's menu under it and Escape returns to the row; the same
  key opens the Explorer's menu for the selection and the editor's menu at the caret. The keys are rebindable
  (`rows.*`, `list.showContextMenu`).
- Screen readers reach the row menus of the Run and Debug view (variables, call stack frames, line breakpoints), the
  Remote Explorer's hosts and the Ports panel: each such row offers "More Actions…" (and the Show Menu action), which
  opens the same menu a right click opens, under the row; the menu takes the keyboard and gives it back when it
  closes. A row that changed since the tree was read opens nothing.
- The extension consent sheet is a modal alert dialog for screen readers: its title, where the package comes from,
  the High-risk line and warnings, every line of the sheet, and Install and Cancel (focused as the keyboard is).
  Escape, Cancel and Install give the keyboard back to what had it before the sheet; a button of an older sheet
  answers nothing.
- Rows of the Run and Debug, Testing and Remote Explorer views and of the Ports panel report the bounds they are
  painted at (clipped to their list), so a screen reader's highlight and hit testing find them.
- The Run and Debug, Testing and Remote Explorer views and open context menus are available to screen readers:
  configurations and Start, the session's state, variables (expandable), watches, call stack and breakpoints with
  their enabled state, the debug toolbar's buttons; test files and tests with their outcome and Run and Debug Test;
  hosts with their connection state and Connect and Disconnect; menu items with their shortcut and disabled state.
  Every action goes through the pointer's path, and an action from an outdated tree does not reach another session,
  test, host or menu.
- The Search side bar's results, the bottom panel's tabs and the Output text are available to screen readers.
- The Settings page, the Keyboard Shortcuts page and the Ports panel are available to screen readers: settings with
  their values, switches, options and fields (changed and checked as with the pointer), commands with their keys, and
  forwarded ports with their state and Open in Browser, Copy Address, Stop Forwarding and Forward Port buttons. An
  action from an outdated tree does not reach another setting, command or port.
- The Problems panel is available to screen readers: each problem with its severity, file, line and column, opened by
  activating it, and a spoken "No problems." when there are none.
- Accounts for extensions: `auth.session` and `auth.fetch` with the built-in GitHub provider (the GitHub CLI sign-in
  Codeyard already uses) or an `[[oauth]]` provider from the manifest (browser sign-in with PKCE through
  `codeyard://ext/<id>/callback`, refresh tokens). The first use asks in a system dialog that names the publisher and
  the account; tokens stay in the system keychain under the extension's publisher identity and Codeyard signs requests
  to the provider's hosts only, so the extension never sees a token. The extension page lists accounts with
  **Sign Out**. Extensions with `secrets` get their own keychain entries, and `net.fetch` reaches the hosts in `net`.
- Extension views: a code extension's `[[views]]` appear in the sidebar (an activity bar button), the bottom panel (a
  tab) or the editor area (a page), and in the palette as *Open View: …*. Codeyard draws the 22 elements itself with the
  theme's colours and spacing, so a theme change redraws them without calling the extension; lists and tables of
  10,000 rows draw only the rows on screen, and trees load one page at a time. The tree arrives as JSON
  (docs/extensions/reference/ui.md), is checked, and is diffed by key so typed text, ticks, selections and expanded
  items survive a re-render. Every element is reachable by keyboard and announced with its role and label. Each view
  names its publisher, and none can cover the consent sheet or take the keyboard from it.
- Extension views show `image` elements with an `https` URL: Codeyard fetches them through the extension's `net`
  permission (declared hosts only, ext-net's address checks, at most 1 MB, formats GPUI decodes), only while the view
  is on screen and never in an untrusted folder, and keeps them by URL until the view closes. A refused image shows
  its alt text and the denial is listed in the Inspector.
- A view's `when` in the manifest: a view is offered in the activity bar, the panel tabs and *Open View* only while
  its `when` holds, and an open view closes when it turns false.
- `views.keystroke_frame_ms` (`CODEYARD_PERF`): a key that reaches an extension view, to the frame that shows it. The
  row in `perf/metrics.json` is `warn` until `scripts/perf/run.sh` has a scenario for it.
- What code extensions contribute reaches the same places as the core's: commands in the editor and file-tree menus
  (with `when` and `enablement`), on key bindings and on status bar items; formatters for *Format Document* and format
  on save (chosen per language with `editor.formatters`); code actions in *Quick Fix…*, whose edits change the document they were asked for and other files only within the
  extension's `write` (one refused file refuses the whole action); diagnostics in the Problems view
  and the editor, tagged with the extension; gutter, line and after-the-line decorations in theme tones; and tasks in
  *Tasks: Run Task…*. Providers answer within a deadline and are cancelled when you type, so they never hold up typing;
  a late answer is dropped and noted in the extension's log. What an extension showed goes when it is disabled,
  uninstalled or crashes. The manifest gains `[[menus]]`, `[[status-bar]]`, `[[formatters]]`, `[[code-actions]]`,
  `[[diagnostics]]`, `[[decorations]]` and `[[tasks]]`, checked like the other contributions.
- View-title menus: `[[menus]]` entries with `location = "view-title"` appear under a **…** button in the title bar
  of that extension view (sidebar, bottom panel or editor), with `when` and `enablement`, and run the command with the
  view's id as its argument.
- Core commands for extensions: a public list (docs/extensions/reference/core-commands.md, generated from
  `ext_manifest::core_commands`) of Codeyard commands an extension may run with `commands.execute("core:…")`: Zen mode,
  Settings, the file explorer, reveal the active file, Problems, Save All, Format Document, open a file and run a task.
  Each names the permission it implies, which the extension must hold too (Save All: `write` on every unsaved file;
  opening a file: `read` on it; a task: its program in `run`, and a single program, not a shell line), checked with the
  permissions in effect in that window. A command not in the list is refused with the fix, and `cyx check` warns about
  one in `permissions.commands`. Menus may name listed core commands that take no argument.
- `codeyard --install-extension FOLDER` installs a built extension folder as it is, without building it.
- Performance scenarios for typing with two decoration providers and for the provider platform overhead
  (`CODEYARD_PERF_DECORATIONS`, `CODEYARD_PERF_PROVIDERS`, run by `scripts/perf/run.sh` with a small decoration
  provider installed twice); both rows stay warn-only until a baseline has them.
- Code extensions run: their commands appear in the command palette, and the first one starts the extension in its
  own `codeyard-exthost` process. Every file, search and process call is checked against the extension's declared
  permissions and the folder's trust before it reaches the folder, locally and over SSH; a denial says which line to
  add. An extension that crashes three times in a minute is turned off until Codeyard restarts.
- Extensions with code: a switch per declared permission on the extension's page, applied at once to a running
  extension (its next call is refused with `revoked-by-user`) and kept per extension and publisher; a list of this
  session's denials with *Copy fix*, which copies the manifest line that allows the call; and a crash card after three
  crashes in a minute with *Reinstall Previous Version*, *Copy Report* (version, error and frames, no file contents or
  outside paths) and *Report…*, which opens a filled-in issue in the extension's repository. Nothing is sent by itself.
- *Developer: Extension Inspector*: per extension with code, when and why it started, its calls with extension, I/O
  and overhead time, denials with *Copy fix*, its log and its last error with frames. Works by keyboard and with a
  screen reader.
- The extension API `codeyard:api` 0.1.0 (docs/extensions/reference/api.md) and its import check: `cyx check`, the
  registry and the installer refuse a code extension whose `extension.wasm` imports an interface its manifest has no
  permission for, with the line to add.
- TypeScript code extensions (docs/extensions/typescript.md): `@codeyard/extension` (`sdk/typescript`) with the same
  API as the Rust SDK, every call returning a Promise, and a global `fetch` mapped to the host's `net` with streamed
  bodies, so npm packages such as Octokit and zod work once bundled. `cyx package`, `cyx test` and `cyx dev` build a
  `.ts`/`.js` entry with esbuild and ComponentizeJS, which `cyx` installs at pinned versions in its cache; Node.js is
  the only requirement. A Node.js built-in import fails the build at its line. `cyx test` shows TypeScript files and
  lines in stack traces. `@codeyard/extension/testing` is the mock host for `npm test`. `cyx new command|view --lang
  ts` and three examples in `examples/extensions/`.
- `codeyard-exthost --compile --cache FILE` reuses Cranelift's compiled functions from the last compile; `cyx test`
  uses it, and a rebuilt TypeScript extension compiles in about half the time.
- Tasks run as their task file describes them, locally and over SSH: process tasks keep each argument as written, shell
  tasks quote for their shell, `dependsOn` runs in parallel or in sequence with a shared dependency once, `promptString`
  and `pickString` inputs are asked first, background tasks release their dependents when their `endsPattern` matches,
  and problem matchers (built-in or custom, multi-line) fill the Problems panel. Each command has its own output
  terminal; "Tasks: Terminate Task" stops everything a run started and "Tasks: Restart Running Task" starts it again.
- A debug configuration's `preLaunchTask` from a task file must succeed (or a background task become ready) before the
  debugger starts; a failure never starts debugging.
- A side view (Explorer, Search, Source Control, Run and Debug, Testing, Remote, Extensions) can move to a secondary side
  bar on the right of the editor and back ("View: Move View to Secondary Side Bar"); its icon and commands then open
  it there, and the arrangement is restored at the next start.
- "Terminal: Move Terminal into New Window" moves the active terminal, with its running process, into a window of its
  own; closing that window returns it to the panel instead of ending it.
- "Terminal: Move Terminal into Editor Area" moves the active terminal, with its running process, into the focused
  editor group as a tab; closing that tab or "Terminal: Move Terminal Back to Panel" returns it to the panel still
  running.
- Terminal tabs can be dragged to another editor group, and screen readers list them with the focused group's tabs.
- Terminals that were in the editor area or in their own window come back in the same place after a restart, as new
  shells with their name, colour and folder and a dimmed notice; no command of the previous session runs again.
  Windows whose display was removed open on an attached display.
- "View: Move Editor into New Window" shows the active file in a window of its own, and "View: Copy Editor into New
  Window" does so while keeping its tab. It is the same document as in the tabs: edits, undo, unsaved changes and saving
  are shared, while each window keeps its own cursor and scroll. Language features, file watching, hot exit backups and
  renames cover it like a tab. Closing that window puts the file back as a tab; ⌘W in it closes the file and asks to
  save when no other tab or window shows it. "View: Move Editors Back to Main Window" returns all of them, and the
  windows come back in place after a restart (a file that is also opened as a tab keeps its tab). The command palette,
  the right-click menu, Go to Line and the other commands and shortcuts work in that window and act on its editor.
- "View: Move View into New Window" shows the open side view (Explorer, Search, Source Control, Run and Debug,
  Testing, Remote Explorer, Extensions) in a window of its own, the same view with the same state. Its icon and
  commands bring that window forward; the command palette, shortcuts, menus and pickers work there. Closing the window
  or "View: Move View Back to Side Bar" puts it back in the side bar it came from. The windows are saved with the
  folder and come back with it, on an attached display; screen readers find the view in its window.
- A side view moves to the other side bar by dragging its activity bar icon there; while it is dragged, both sides
  show a highlighted drop zone. The panel docks left of, right of or below the editor ("View: Move Panel Left",
  "View: Move Panel Right", "View: Move Panel to Bottom"), and the dock is restored at the next start. Screen readers
  read which view each side bar shows and where the panel is from the window's description.
- Screen readers see the editor in its own window (title, text, caret and selection, find bar, suggestions, the
  picker opened there) and the terminal in its own window, as in the main window.
- Terminals come back after a restart as new shells in their folders, with their titles, colors and programs, and say
  so; nothing runs again. `terminal.persistentScrollback` (off by default) also keeps the last lines of their history as
  plain text; "Terminal: Clear Saved Terminal History" removes it.
- Extensions with code ask once per session, in a system dialog that names the extension and its publisher, before
  their first clipboard read and before the first program they run on a remote host. **Deny** refuses it with
  `revoked-by-user` until Codeyard quits, and the refusal is listed under the extension's recent denials.
- On macOS and Linux an extension process that uses more than 512 MB of memory is stopped and counted as a crash, with
  a message that says why (Windows already had the limit).
- `cyx package --no-build` packages the `extension.wasm` already in the folder.

### Changed

- The settings and shortcuts import is "Preferences: Import Settings and Shortcuts from Another Editor", and its
  messages no longer name another product. A shortcut block written by an earlier import is still replaced by a new one.
- Installed extensions with code start no process when Codeyard opens: the check that the system sandbox works now
  runs when the first one starts. The startup measurement with 100 installed extensions now has 50 with code.

- A task no longer types its command into a new terminal: it runs as a process on the folder's machine, so a value from
  a variable can never become more shell code, and Stop ends the processes it started.
- The Linux agent's size budget is 1664 KB (was 1600 KB): running tasks as processes on the server needs its own
  request in the protocol.

### Fixed

- A workspace file's debug configurations understand `${workspaceFolder:name}` and `${workspaceFolderBasename:name}`, as
  its tasks already did. Before, the program path kept the variable as written and debugging failed to start with
  "Expected process to be stopped".
- Windows of their own (editors, side views, terminals) on macOS came back 32 pixels taller at every start: the frame's
  height was saved and the content was opened at that height. The content's size is saved now.
- A file moved into a window of its own came back as a tab of the main window too after the second start when Codeyard
  had been killed or had crashed: the tab loaded at the first start to show it was saved as a tab.
- Opening a folder with `--terminal`, or asking for a new terminal while the folder's terminals were being restored,
  made the restore give up: the saved terminals did not come back and the new shell ran outside the terminal session,
  so it was not kept for the next start. The new terminal now waits for the restore.
- Windows: a shell task's arguments run through cmd.exe reach the program unchanged when they contain `%` (a value like
  `%PATH%` is no longer expanded) or end with a backslash (`C:\dir\` no longer swallows the closing quote).
- Screen readers find the Search results in the secondary side bar too, not only in the primary one.
- Screen readers find an extension's side bar view when it is in the secondary side bar.
- On macOS and Linux, a task, test run, language server or terminal no longer keeps running when Codeyard is killed or
  crashes. A small guard process started with the app ends every task's process group at once and hangs up every
  terminal, killing it after two seconds if it is still there. Windows already ends them through job objects and the
  pseudo console.

### Security

- `wasmtime` and `wasmtime-wasi` 50.0.1 in `codeyard-exthost` (RUSTSEC-2026-0420, RUSTSEC-2026-0422,
  RUSTSEC-2026-0423, RUSTSEC-2026-0424): fixes for typed functions with `i31` or `nullref` arguments, cross-engine
  struct types, resources passed between components and the WASI host. Each extension still runs in its own exthost
  process inside the OS sandbox.
- `wasmtime` and `wasmtime-wasi` 50.0.1 for the extension host (RUSTSEC-2026-0420, RUSTSEC-2026-0422,
  RUSTSEC-2026-0423, RUSTSEC-2026-0424): type confusion between engines, a panic on typed functions with `i31` or
  `nullref` arguments, and a UDP send to an address only allowed for receiving.

## [0.6.0] - 2026-10-29

### Added

- Search Editors: "Search: Open Search Editor" keeps a query in its own tab with its options, globs, context lines and
  folders (every folder of a multi-folder workspace), for keyboard navigation and rerun. Tabs return after a restart
  with their query and up to 200 results, marked as restored until they run again; truncated, stopped and failed
  searches say their results are incomplete.
- Replacement review: Replace in the Search panel opens a preview of every change, selectable per change and per file,
  made from open editors' unsaved text where there is one. Apply writes each file atomically only if it is unchanged
  since the preview, keeps encodings, byte-order marks and line endings, refuses incomplete results, and reports per
  file what changed, what did not and what is uncertain. One "Undo Replace" takes the batch back and reports files
  edited since as undo conflicts.
- Workspace files are kept whole. Folder names, relative and absolute paths, and the shared `settings`, `tasks`
  and `launch` sections are read. Adding or removing a folder rewrites only the folder list, so comments, unknown
  fields and other sections stay as written; nothing is written when nothing changed. *File: Save Workspace*,
  *Save Workspace As…* (relative paths are rewritten for the new place), *Reload Workspace* and *Open Active File
  as Workspace* (for files on an SSH server) are new. Writes are atomic through the host, locally and over SSH. A
  file changed elsewhere is not overwritten (*Workspace changed on disk. Reload before saving.*). An unreadable file
  is reported and never replaced. Missing folders stay listed in the Explorer with Retry and Remove.
- Settings precedence across a workspace is now user < workspace file < folder, for the first folder and for every
  extra folder; one folder's settings no longer leak into another's files. The Settings page shows when a value
  comes from the workspace file or the folder. Tasks and debug configurations from the workspace file run in the
  active folder. Trust is asked for that folder and, when the file lives elsewhere, for the file's folder too.
  A workspace file has its own Recents entry; reopening it (or a restart) brings back its folders, order, names and
  settings, while opening one of its folders directly opens only that folder.

- Interface scale: *View: Zoom In*, *Zoom Out* and *Reset Zoom* (⌘= / ⌘- / ⌘0, Ctrl on Windows) scale the whole interface
  from 75% to 200% in steps (75%, 80%, then every 10%), including menus, dialogs, tooltips, hit regions and accessibility
  bounds. The value is saved once per change as `ui.scale` in the user's settings, applies to every open window and is
  shown as a localized percentage. Invalid values fall back to 100% with "Interface scale must be between 75% and
  200%."; a project settings file cannot set it. The display's own scale stays separate. Typed values are rounded
  to whole percent; a zoom that cannot be saved stays for the session.
- Source Control manages several repositories in one workspace. A bounded background search finds the repositories in
  the workspace folders and nested inside them; the changes list groups them by repository with each one's branch,
  ahead/behind, upstream, operation in progress, conflicts and change count. File and group actions act on their own
  repository (identical relative paths stay apart); commit, sync, history and the "…" menu act on the selected
  repository, which keeps its own draft and result. Operations carry the repository's identity and are checked again
  before Git runs, so a repository removed or re-created while one is pending is never acted on, and its result is not
  shown for another. Keyboard (↑/↓, Home/End, ←/→) and screen reader access to the repository list. Works locally and
  over SSH. History, stash, branch-picker and named-stash actions stay with the repository they were opened on.
  Repositories refresh a few at a time, and the workspace is searched again only when folders change, a repository
  disappears, or on Refresh after a minute. The agent protocol is version 14: `Stat` also reports a file's identity and
  creation time, which identify a repository's git directory.
- The extension registry's signed index: format, offline verification (a threshold of root keys, an expiring online
  key, no rollback to an older copy) and `cyx yard keygen | sign | verify` for the key ceremony and test registries
  (docs/registry.md).
- Offline verification of publishers' keyless Sigstore signatures, pinned to the registry's Fulcio CA and Rekor key,
  with the publisher identity (repository, owner and workflow by id) taken from the certificate.
- Install from a registry: *Extensions: Install from Registry…* and `codeyard --install-extension publisher.name[@x.y.z]`.
  The signed index, the package and the publisher's signature are checked before the consent sheet, which names the
  publisher; the lock pins it, and a new publisher or an index older than 3 days is called out. Registries besides the
  public one go in `extensions.registries`. No request is made until something is installed from a registry, then at
  most one a day.
- The Market: a second tab in the Extensions view that searches every registry's extensions on the device as you type
  (10 000 in well under a frame), with a page per extension: README without remote images, versions with their
  status, permissions in plain words, the publisher and downloads including updates. Install goes through the consent
  sheet. Opening the Market reads each registry's list at most once a day; searching sends nothing. The registry
  serves each version's manifest and README as a details file whose digest is in the signed index.
- Running the public registry: `yard init --root-keys` (no root key on the server), `yard rotate` and `yard promote`
  for the 90-day online key with the root key holders' signatures, `cyx yard sign --key -` for a key kept in a
  password manager, `cyx yard check` for a live registry, ETag answers for the index, the root key ceremony script,
  the server setup (`deploy/yard`: systemd, Caddy, Litestream and R2 backups, single-command SSH keys), and hourly
  monitoring, a weekly revocation canary and deploys from GitHub Actions. Hosting and the runbook are in
  docs/registry.md.
- Extension updates and version status: `extensions.autoUpdate` (`install`, `notify`, `off`). Data-only versions
  update at once and versions that run code after 48 h (*Update Now* skips the wait); more permissions wait for a card
  that lists only what is new, and another publisher for the full sheet. A withdrawn version goes back to the newest
  earlier one at once, and a revoked one stops with its dependents, also offline and after a restart. Pre-releases are
  opt-in per extension on its page.
- `cyx publish` signs keyless (GitHub Actions' identity, or a browser sign-in) and publishes; `--dry-run` prints the
  install sheet in the app's words. `cyx yank`, `restore` and `deprecate` change a published version. `cyx new` writes
  `.github/workflows/publish.yml` for tag-push releases through the `codeyard/publish` action. `.github` is never packaged.
- `yard`, the registry's publish service: signed uploads with no accounts, the same checks as `cyx check`, publisher
  names that belong to their first owner, confusable names refused, versions never reused, yank/restore/deprecate as
  signed statements, revoke and advisory fixes for the security role, and a static index signed daily.
- Paste clipboard images into programs such as Codex in an SSH terminal: the image is transferred to a private temporary
  file on the remote host and its path is pasted into the terminal.
- Local SSH port forwarding on Windows using a dedicated OpenSSH process for each forward.
- Explicit local/SSH setup for JavaScript/TypeScript, HTML and CSS language services, with pinned
  packages, a checksum-verified Node.js fallback, progress, cancellation, retry and atomic activation.

- Nested horizontal and vertical editor groups with directional splitting/focus, independent views of
  shared documents, tab transfers, nested divider resizing and atomic layout/session restoration.
  Up to 32 groups retain usable minimum sizes with scrolling in narrow windows.

- Explorer multi-selection with modifier clicks, keyboard ranges and select-all, plus batch copy,
  cut, drag and deletion with a single confirmation, progress/cancellation and per-entry errors.
  Failed cuts remain available for retry; dirty buffers and successful moves retain their edits.
- Filesystem-native recursive copies on local and SSH agents, with exclusive atomic publication,
  symlink and executable-permission preservation, bounded traversal and no shell dependency.

- `codeyard --install-extension FILE.cyx` installs an extension without opening a window (repeat the flag for several).
  The performance gate now measures loading 100 installed extensions at startup (ceiling 5 ms).
- `cyx`, the extension authors' command, ships beside Codeyard: `cyx new` (theme, snippets, keymap, language, language
  server), `cyx dev` (runs Codeyard with the folder loaded and a DEV badge; it reloads on save), `cyx check` (every
  problem with its file, line and fix), `cyx package` (a reproducible `.cyx`) and `cyx import-vscode` (themes, snippets,
  languages and local JSON schemas from a VS Code extension or `.vsix`). *Extensions: Install cyx Command in PATH* links it.
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

- Terminal panel toolbar: New Terminal with a launch menu (profiles and installed shells, Split Terminal, tasks, default
  profile and *Configure Terminal Settings*), Split and Kill buttons, shown only while the Terminal tab is active. Its icons
  share one optical size, and the maximize button shows when the panel is maximized. Maximize grows the panel towards
  the top (the left when docked right) and restore shrinks it back, animated unless motion is reduced.
- Terminal list: resizable by its left edge (remembered), larger rows, animated activity bars while a command runs, dots
  for a failed last command or a long command that finished out of sight, tooltips with folder and command,
  double-click to rename and middle-click to close.
- *Terminal: Focus Next / Previous Terminal* (⇧⌘] / ⇧⌘[), *Terminal: Kill Other Terminals* and split in the terminal
  with ⌘\ (Ctrl+Shift+5).

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

- The status-bar resources open as a strip inside the status bar, sliding left from their icon, instead of a card above it.
  The most important figures sit next to the icon, so a narrow window cuts the others first.
- Focus rings show only after the keyboard was used: a click lights the button it presses instead of outlining it.
- Replace All and "replace in this file" in the Search panel open the replacement review instead of a confirmation that
  wrote at once. Saving against a known version no longer recreates a file deleted since (it is a conflict); a file the
  editor knows was deleted is still saved back. "Search: Open Results in Editor" is now "Search: Open Search Editor" and opens a Search Editor tab instead
  of a plain-text file.
- Zoom no longer changes the editor and terminal font sizes for the session: it scales the whole interface, and
  `editor.fontSize` and `terminal.fontSize` keep the values you set. The activity bar's view buttons scroll when the
  window is too short for all of them, with a chevron at the edge where more are hidden, and the Manage button stays
  visible.
- The patched GPUI applies a logical interface scale at the window (`vendor/gpui/src/ui_scale.rs`, listed in
  `vendor/gpui/PATCHES.md`).
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

- Port mappings and mounted storage use responsive inspection cards with copyable addresses and paths.
  Log entries place timestamp and severity above full-width messages, with a full-width search and console footer.

- Container details promote the selected name and file/terminal actions into the header. Responsive CPU/memory
  cards show current readings above charts, disk/network activity shares the available width, and log search has a visible field frame.

- Container Overview fills the available width when resizing or hiding columns. Name and Image expand while
  metrics stay aligned with larger table text; header actions and search/filters wrap in narrow windows. Sorting moves to the
  Name/CPU/Memory column headers, and Clear filters appears only for an active query or filter.
- The Container Overview header combines connection, update state and fleet counts, with grouped filters
  and a framed search field. Redundant initial navigation and duplicate count rows are removed.

### Fixed

- The extension host's idle memory for a TypeScript extension drops from 54 MB to 22 MB on macOS: it hashes the
  compiled code in pieces and copies the code to executable memory without a heap buffer, so 32 MB of freed memory no
  longer stays with the allocator.
- The divider between editor groups no longer jumps away from the pointer when the interface scale or the window size
  changes while it is being dragged.
- The publish workflow that `cyx new` writes pins `actions/checkout` and the Codeyard publish action to commit SHAs. It
  referred to a `v1` tag that did not exist, and a tag can be moved to code that would then run with the job's signing
  identity.
- Terminal Space sends a space, and Ctrl+C copies a selection while still interrupting the program when nothing is selected.
- SSH connections detect an incompatible bundled agent before attaching to its daemon and explain how to rebuild or reinstall it.
- Stopping a piped process also kills descendants when its leader has already exited. A process
  that closes stdout remains cancellable instead of blocking the agent until its natural exit.
- Separate SSH configuration files use separate control connections even when they define the same host alias.
- Web Preview reads files through directory handles without following symlinks or reparse points,
  preventing pathname replacement races. Each file now needs one agent request, reducing SSH round trips;
  directory and session redirects preserve URL query parameters.

- Opening or selecting a file restores a maximized terminal panel to its previous size so the editor is visible.
- Picker inputs support Select All, Copy, Cut and Paste, including remote folder paths and SSH password paste.
- Switching SSH hosts resets the workspace even when both servers use the same folder path, and preserves the
  previous server's saved tabs. Disconnecting clears remote folders, Git state and project settings without overwriting its session.
- The empty Explorer's workspace link stays on one line in narrow sidebars.
- Moving a maximized panel to the right or bottom did nothing visible; it now moves and returns to its own size.
- Picking the other half of a split terminal, or another terminal and back, no longer drops the split, and closing a
  terminal listed before the active one keeps the active terminal selected.
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

### Security

- `libz-sys` 1.1.31 (RUSTSEC-2026-0418): its build script no longer writes the build's environment under `target/`. No
  release or CI build had secrets in that environment.

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

[Unreleased]: https://github.com/doancan/codeyard/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/doancan/codeyard/compare/v0.5.0...v0.6.0
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
