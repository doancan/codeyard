# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `cyx test` asks every declared provider once (formatter, code actions, hover, completion, code lenses, inlay hints,
  tasks, source control) with a sample document, reads workspace files from `files` in `test/fixtures.json`, runs
  commands with the arguments in its `commands`, and starts the extension host again after a trap or a timeout, so one
  run reports every problem. A trap shows the extension's own frames first, with Rust names demangled.
- `MockHost::decorations(path)` (Rust); `on_settings_changed` and `every` handlers may return `Result<()>`, so `?`
  works in them. The TypeScript `ctx.onViewEvent` handler gets a typed `ViewEvent`.
- docs/extensions/reference/globs.md: how `read`, `write` and `workspace-contains` globs match.

- Extension API 1.1 (`codeyard:api@1.1.0`), additions only: components built against 1.0 run unchanged, and an
  extension that uses nothing new still needs only API 1.0. Both ship in this release.
  - Watches: `ctx.on_documents` (TypeScript `ctx.onDocuments`) reports documents opening, changing (once typing
    pauses, with the version, the text only when asked) and being saved; `ctx.on_files(glob)` (`ctx.onFiles`) reports
    workspace files created, changed and deleted, by Codeyard or anything else, at most one batch a second. Both need
    `read`, and an event comes only for a path the extension may read. A TODO list no longer has to poll.
  - `ctx.open_in_editor(uri, selection)` (`ctx.openInEditor`): `core:editor.open` also takes
    `{"uri": …, "selection": range}`, so a view can jump to a line.
  - `[permissions] net-from-setting = "<setting>"`: an extension that talks to a server the user names in a setting,
    such as a GitHub Enterprise host, reaches it once the user allows the host on the extension page (Servers ›
    Allow Host…, confirmed in a system dialog). Nothing the extension, a workspace or a setting writes allows a host.
  - The Rust and TypeScript mock hosts open, edit and save documents, change files and allow hosts; `cyx test` gives
    each watch a file to watch and treats the host in a `net-from-setting` setting's default as allowed.

- The Rust SDK has `read_many`, `search`, `rename` and `create_directory`, and the TypeScript SDK `readMany`, `rename`
  and `createDirectory`, as the API has them. `read_many` reads many files in one round trip: 2000 reads one after
  another take about 2.5 s.
- `doancan/codeyard/actions/setup-cyx`: installs `cyx` and the extension host in CI, built from the action's commit.
- `cyx new` pins a Rust extension's SDK to the tag of the `cyx` release that wrote it (a preview `cyx` before 0.7.0
  follows `main`, since no release has the SDK yet). The contributions reference lists the built-in language ids and
  key names.
- Screen readers reach the editor's drawn controls: gutter breakpoints, fold buttons and Git marks, sticky scope
  lines, merge-conflict links and Send Request; the color picker's hue, saturation, brightness and opacity as
  sliders; the terminal's find bar; the terminal's history by scrolling; and the update window on macOS has its own
  accessibility tree. Expand and Collapse from Windows UI Automation work on every node that folds.

- Screen readers read and drive the pages: the merge editor (each conflict, both sides' text and every accept
  button), the folder comparison (options, rows with their status and the copy and delete buttons), the diff and the
  large-file view line by line, the Markdown preview's headings, paragraphs and links, the Welcome page's tiles and
  recent folders, the data query bar and the containers' overview, details and logs.

- Screen readers reach every button drawn in the side views: the Explorer's header buttons, section headers, open
  editors, favorites, outline and timeline; Source Control's Refresh, More Actions and stashes; the Search view's
  result buttons and structural language; Run and Debug's Remove All Breakpoints and launch link; the Testing filter;
  the Remote Explorer's folder and port buttons; the Extensions header and an installed extension's whole page,
  including its permission switches, denials, crash card and accounts. Each reports the frame it is drawn at and does
  what its click does.

- Screen readers reach the window's chrome and the controls drawn around the editor: the activity bar's views and its
  Manage button, the title bar's Back, Forward and file search, the caption buttons on Windows, every status bar
  item (each one does what its click does, including sync, ports, transfers, the terminal toggle, conflicts and the
  resource monitor with its figures), the error card's Reconnect and Close, breadcrumbs, the menu bar on Windows and
  Linux, the panel's buttons and its list of terminals, the editor tabs' Close, Unpin, Read and Preview buttons, the
  diff, large-file and page tabs, the tabs of every other editor group, the Remote Explorer's Add, SSH config and
  Refresh buttons, the Run and Debug configuration selector and a breakpoint's Go to Breakpoint. Each reports the
  frame it is drawn at.

- Every confirmation dialog marks its cancelling button for the system, whatever its wording or language, so Escape
  answers "Not Now", "Keep", "Keep Waiting", "Keep Going", "Deny" and a translated "Cancel" on macOS and Windows; if
  something took the keyboard while a question was open, cancelling gives it back to where it was.

- Screen readers reach the rest of Source Control, the editor's popups and the Debug Console: the Sync / Publish
  button; the Worktrees section, whose checkouts open from the tree; the Graph with its buttons (search, fetch, pull,
  push with their counts, reload, its menu), filters, commit search and commits, which select, open and open their
  menu; the selected commit's message and files, which open their diffs. The hover and signature help are read-only
  tooltips under the editor (Show Hover from the screen reader shows the hover; its related locations are links), and
  the Debug Console is a live log with its expression field and an Evaluate button. Every one of them reports the
  frame it is drawn at.

- Screen readers and magnifiers get the on-screen position of more of the interface: each Extensions row (Installed
  and Market), the picker's query box, list and rows, each suggestion, the Settings page's search field, categories,
  options, Reset buttons and page buttons, the bottom panel's tabs, the Search Editor's query, status, results and
  buttons, Search Editor and terminal tabs, and the update dialog's buttons. Right-to-left and mixed-direction text in
  the editor reports each character where it is drawn, so a magnifier or a screen reader's cursor lands on the right
  letters.
- Screen readers can use the whole Search view and fix problems from the Problems panel: the query (typing from the
  screen reader searches), its options as check boxes, the replacement, Replace All, the include/exclude fields, the
  result line (a search or pattern error is announced at once) and the results as a tree of files and matches that
  expand, collapse, open, and offer Replace in this file and Dismiss. Each problem has a Quick Fix… button that opens it
  and lists its fixes in a picker; the problem counts are announced as they change. Search and Problems rows report the
  frames they are drawn at.
- Running the public registry (#70, everything that needs no server): `deploy/yard/setup.sh` sets up a fresh Ubuntu
  24.04 server behind Cloudflare (origin certificate, HTTPS from Cloudflare's addresses only, SSH by key) with one
  secrets file (`yard.env.example`), a daily `yard resign` timer, an hourly check (`yard-watch`), alerts by webhook or
  e-mail when a job fails or the index is within 24 h of expiry (`yard-alert`), daily backups to R2 beside
  Litestream's continuous copy, a restore script and a weekly restore check, and journal rotation. `yard status`
  prints the index's and online key's expiry and fails within 24 h / 14 days; `GET /metrics` gives the same in
  Prometheus text (local only); `yard init --fulcio --rekor` pins another Sigstore and `yard resign --expired` signs an
  already expired index, both for staging and rehearsals. The canary keeps each run's result in `canary.tsv` on the
  `yard-canary-log` branch for the release notes. `scripts/yard-root-ceremony.sh` writes a fuller record (the lines
  for the server, the app and GitHub, checks and the owner's sign-off) and has a `--rehearsal` mode; the build refuses
  placeholder or malformed public registry keys. `deploy/rehearse.sh` runs ceremony, publish, install, revocation and
  an expired index end to end on one machine, also in CI. docs/registry.md has the runbook (key rotation, revocation,
  advisory wait, restore, inviting a publisher, reviewing a first native program, rotating the canary key, cost,
  moving hosts) and docs/registry-setup-checklist.md what the owner provides and the go-live order.
- `codeyard --update-extensions`: the extension update check without a window, asking the registries of installed
  extensions now; it updates, rolls back and stops revoked versions as the daily check does, and prints what changed.
- Screen readers open an Explorer row's menu from the row (`ShowContextMenu`, or its "More Actions…" button), as the
  other lists' rows do, and the Settings page says a rejected value at once. Confirmations reached from the tree
  (Delete from a row's menu, Discard, closing an unsaved editor, a settings change) leave the keyboard on the row or
  field they came from when cancelled, and a failure after one is announced at once.
- Screen readers and magnifiers get where things are drawn, not only the main areas: Explorer rows, the editor tabs,
  the status bar's items, the find bar's fields and buttons, Source Control's message field, commit button, groups,
  files and their buttons, and the buttons inside Run and Debug, Testing, Remote Explorer and Ports rows each report
  their own frame, and every character of the editor and the terminal on screen has its position, so the caret and the
  selection have bounds (VoiceOver's and Zoom's `AXBoundsForRange`, UI Automation's text ranges). Frames follow the
  interface scale and stay right in an editor's or a view's own window; a side view in its own window no longer leaves
  its rows' frames in the main window's tree, and Testing and Remote Explorer are in the tree when they are shown in the
  secondary side bar or a window of their own. `scripts/a11y-probe/axframes.swift` prints and draws one process's
  frames.
- Source Control's change list opens a file's right-click menu from the keyboard: Shift+F10 or the Menu key on the
  cursor row shows it under the row, the arrows and Enter drive it, and Escape gives the keyboard back to the same row.
  Screen readers get `ShowContextMenu` and a "More Actions…" button on each file, as the other lists' rows have.
- Performance scenarios for extension views and hot reload, run by `scripts/perf/run.sh`: PageDown in a sidebar view
  with a list of 10 000 rows (`CODEYARD_PERF_VIEWS`, `views.keystroke_frame_ms`) and a 2 MB Rust component swapped in
  place in a dev window (`CODEYARD_PERF_RELOAD`, `extensions.hot_reload_ms`), each with a fixture extension that needs
  no permission and no WebAssembly toolchain at run time. Both rows stay warn-only until a baseline has them.
- Native programs from packages (#146): an extension's `run` may name a program inside its package (`./bin/hello`),
  built per platform under `native/<triple>/` (static musl on Linux). `cyx package` writes one
  `<id>-<version>-<triple>.cyx` per platform (`--target` for one; up to 128 MiB each) and `cyx publish` uploads each.
  The registry index and `extensions.lock.jsonc` (format 4) carry every platform's digest; Codeyard installs the
  workspace host's platform, fetches another one when a window on that platform needs it, copies the program to the
  host with the agent install's temporary file, `sha256sum` and `mv` steps, and checks its hash before every run,
  locally and over SSH. The consent sheet asks for the extension's id to be typed before it installs one. Yard takes
  code and native programs only from publishers listed in `yard.json`'s `invited`, and a publisher's first native
  program stays unlisted until a reviewer runs `yard review approve` (`yard review list | approve | reject`).
  A `[[language-servers]]` command inside the package (`command = "./bin/server"`) starts from the same pushed and
  hash-checked copy, locally and over SSH, and is refused when the hash does not match. An update that adds a native
  program opens the full sheet, which asks for the id to be typed as a first install does, instead of the permission
  card. Screen readers find the sheet's id field (a text field with what to type, what is typed and the focus).
- "Preferences: Apply Rust Setup…" sets up a Rust project in one step: Clippy for checks, checks with the Cargo
  features Run and Debug build with, a check folder of its own and rustfmt on save. It shows the exact changes to
  `settings.jsonc` first, keeps your comments and other settings, and "Preferences: Reset Rust Setup…" removes it.
- Rust debugging builds with your choice of Cargo features, profile and output folder: `rust.features`,
  `rust.noDefaultFeatures`, `rust.profile` and `rust.targetDir`. The target picker shows them next to each detected
  target, and a relative output folder is resolved in the workspace folder of the machine that builds, also over SSH.
- Code extensions can describe and complete: `[[hover]]` and `[[completion]]` (with optional `trigger-characters`)
  in the manifest, `request.hover` and `request.completion` in `codeyard:api` 0.1, `ctx.hover` and `ctx.completion`
  in the Rust and TypeScript SDKs, and `hover` / `completion` in both mock hosts. Their text shows in the hover popup
  under the language server's, and their items join the completion list, each tagged with the extension. They have
  the provider deadline; a completion request is cancelled when its word is left, a hover request when the mouse
  moves on, and a late answer is dropped and noted in the extension's log. Typing never waits for them, and turning
  an extension off or uninstalling it removes its items at once. Over SSH the document comes from the editor. The
  `todo-actions` and `crate-version-ts` examples use them; `typing.keystroke_frame_completion_ms` (warn) measures
  typing with one completion provider.
- Code lenses and inlay hints from code extensions: `[[code-lens]]` and `[[inlay-hints]]` in the manifest,
  `request.code-lens` (each lens with a command and its arguments, `request.resolve-code-lens` for one left without)
  and `request.inlay-hints` in `codeyard:api` 0.1, `ctx.code_lenses` / `ctx.codeLenses` and friends in both SDKs and
  their mock hosts. They show at the end of their line with the language server's, name the extension in the tooltip,
  are asked for the lines in view only (again once typing pauses or the view stops scrolling, cancelling the question
  before), and typing never waits for them. Clicking a lens, or *Editor: Run Code Lens on This Line*, runs its command
  through the broker: the extension's own, a dependency's, or a `core:` command its manifest lists. The `todo-actions`
  example shows both; `crate-version-ts` hints the latest version after each dependency in a `Cargo.toml`. New perf
  metric `typing.keystroke_frame_lenses_ms` (warn).
- Source control providers from code extensions (`[[scm]]`, #144): an extension claims the workspace folders that
  have one of its markers (a `.hg` or `.jj` folder) and Source Control shows its head, input box and groups of changed
  files there, with the extension's own commands on each file, group and the input box; the gutter's change marks
  compare open files with the original text it gives. Git stays built in for every other folder. The API's new `scm`
  interface and the `scm-status`, `scm-original` and `scm-validate` requests are in the WIT, the exthost, both SDKs
  and their mock hosts. The state is asked again when files change, on Refresh, after a button and on `scm.refresh`;
  untrusted folders are not claimed, and the programs a provider runs go through `run` on the workspace host, also over
  SSH. Screen readers reach a provider's section, input box, files and buttons as they do Git's. New example:
  `jj-scm`, a provider for Jujutsu.
- Pull request providers from code extensions (`[[pull-requests]]`: `id`, `label`, the remote `hosts` it claims and the
  `auth` provider it uses). The Pull Requests view lists their open pull (merge) requests next to GitHub's, per
  repository remote; picking one shows its details and comments as a Markdown page, its changed files as diffs, adds a
  comment, or checks it out (git runs on the workspace's host after the provider's answer is checked). Providers call
  their forge with `auth.fetch`, so they never see the token; they have 20 s per query
  (`PULL_REQUEST_DEADLINE_MILLIS`), are cancelled after it, and their errors and late answers go to the Inspector. API
  `codeyard:api@0.1.0` gains the `pull-requests` interface, `request.pull-requests` and `response.pull-requests`
  (additive); the Rust SDK has `ctx.pull_requests(id)`, the TypeScript SDK `ctx.pullRequests(id, provider)`, both mock
  hosts can send the queries, and `cyx test` asks each provider for a list. Example: `examples/extensions/gitea-pulls`.

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
  Closing the window still ends them at once; opening another folder or closing the folder keeps the panel's
  terminals, as without the daemon, until the window closes. Terminals another Codeyard instance has open stay with it:
  the window starts new shells and says so. The agent protocol is version 16.
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
  row in `perf/metrics.json` is `warn` until a baseline has it.
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
- Example extensions built on the public API only, as an outside author would (#148): `vercel-status` (TypeScript: the
  latest deployment in the status bar, recent deployments in a sidebar view, a refresh command and timer, the token in
  `secrets`) and `gitlab` (TypeScript: merge requests in the Pull Requests view through a GitLab `[[oauth]]` account,
  gitlab.com or a self-managed instance). CI builds both and runs their unit tests and `cyx test`.
- Both mock hosts: `secrets()` returns the keychain entries the extension left. An `auth.fetch` to a host outside the
  account's `[[oauth]] hosts` (or GitHub's API hosts) is refused, and one with its own `authorization` header fails, as
  in Codeyard.
- Self-managed forges without repackaging (#184): an `[[oauth]]` table with `instances = true` lets the user add a
  server of their own (a company GitLab, a Gitea or Forgejo) on the extension's page, Accounts › **Add Instance…**,
  with the client id of the OAuth application they registered there. A system dialog names the extension, its
  publisher, the host, the client id and the redirect URI first, as for a permission; instances are kept per
  extension and publisher identity next to the permission switches, and only that page adds them. Each is an account
  of its own (`gitlab@git.example.com`) that signs requests to its host only; the extension's `[[pull-requests]]`
  provider claims its remotes, and `net` covers its host where it covers the table's. The `gitlab` and `gitea-pulls`
  examples use it. The manifest change is additive (#147).
- Sign-in outcomes in both mock hosts (#185): `decline_sign_in` / `declineSignIn` (the user cancels, `auth.session`
  fails with `cancelled`), `signed_out` / `signedOut` (`auth.fetch` fails with Codeyard's "not signed in" error until
  the next `auth.session`), `sign_in` / `signIn` with an account name, and `instance` for a self-managed instance.
- `cyx test` answers `net.fetch` and `auth.fetch` from HTTP fixtures (#185): `test/fixtures.json`, or
  `--fixtures FILE` (method, URL, status, headers, body; docs/extensions/testing.md), after the usual permission and
  host checks, so a provider's success path runs in the real extension host. `cyx test` now also refuses an
  `auth.fetch` to a host outside the account's, as Codeyard does.
- `cyx new pull-requests <name> [--lang ts]` (#187): a pull request provider with its `[[oauth]]` account
  (`instances = true`), unit tests and `cyx test` fixtures, and a README on registering the OAuth application.
- `cyx check` warns about a placeholder OAuth `client-id` (`your-client-id`, `<…>`, `example…`), and `cyx publish`
  refuses one (#187).

### Changed

- Windows CI runs every test of the workspace (`cargo test --workspace`, no name filter); before, the app's tests ran
  through a list of name filters, so most of them, among them every extension test, never ran on Windows and new
  tests were left out silently. A test that needs what the runner lacks says so in a SKIPPED line with the reason:
  the exthost's OS sandbox, a POSIX shell, sshd. macOS and Linux CI set `CODEYARD_REQUIRE_TOOLS=codeyard-exthost`, so
  the extension tests cannot pass there by skipping. Fixed on the way, on Windows: a paused debug line, a file
  opened from the command line or the Open dialog and a folder added to the workspace kept the `\\?\` prefix of a
  canonical path, so they matched no tab, root or breakpoint; they now keep the workbench's form (`C:/...`).

- The mock hosts (Rust and TypeScript) and `cyx test` judge diagnostics as Codeyard does: `set_diagnostics` needs a
  `[[diagnostics]]` declaration that lists the file's language, a file in the workspace or open in the editor, and at
  most 1000 files; messages with bidirectional or zero-width characters are dropped and ones over 4096 characters cut.
  The rule is `ext_manifest::calls` (with `builtin_language` for the file name's language, which the app's table is
  tested against); the TypeScript copy is checked against cases the Rust rules judge. A refusal is a warning in the
  mock's `logs()` and a failure in `cyx test`.
- A path outside the workspace and a request to an IP address are denied in the mocks and in `cyx test` with the same
  `(cannot be granted: …)` line and `none: this cannot be granted, …` fix the app and the SDKs give (a test in `cyx`
  runs both through the real extension host).

- One set of rules for the app, the extension host, both mock hosts and `cyx test` (`ext_manifest::glob`, `allow`,
  `calls`; the TypeScript mock has a copy checked against cases the Rust rules judge): a glob is matched against the
  whole workspace path in linear time (`read = ["main.rs"]` is no longer `src/main.rs` in tests), a path is resolved
  (`..`, `%2e%2e`) before it is matched, request URLs are checked once (`http://`, user info and IP addresses are
  `invalid` in tests too), and `set-status-item` refuses an undeclared id or a text over 64 characters in the extension
  host instead of the app dropping it silently. `cyx check` refuses a leading `!` and `**` inside a path part.
- `window.input-box` with `password: true` shows a masked input instead of failing: a token pasted for `secrets` stays
  possible (docs/plugin-architecture.md §6.4) and is not shown on screen.
- An `async activate` in TypeScript: requests wait for its Promise, and a rejection fails the activation (logged as
  `activate failed: …`; `cyx test` and the mock fail on it).
- The TypeScript mock is the global `fetch` from the moment it is made, and a `fetch` from the test itself is answered.

- The extension rows of the performance gate block a release: typing with decorations, completion, lenses and
  hints, keys in an extension view, the provider overhead (ceiling 3 ms) and hot reload (ceiling 500 ms). A keystroke
  row's ceiling is now relative (`ceiling_vs` in `perf/metrics.json`): the same keys typed without extensions in the
  same run, plus 10%, instead of a fixed 8 ms that a time including up to two display refreshes could never meet.
  New row `typing.keystroke_frame_words_ms` (`CODEYARD_PERF_WORDS`): the completion scenario's keys without an
  extension, its reference. The reference record is `docs/benchmarks/perf-2026-10-31.json`.
- The extension API is `codeyard:api` 1.0.0 (#147), with the TypeScript SDK `@codeyard/extension` 1.0.0. From 1.0
  a minor only adds: every interface, function and type carries `@since(version = …)`, the API reference shows it,
  and a test in ext-api compares the WIT with the released copy in `crates/ext-api/released/1.0.0` and fails on a
  removed or changed item (a new field or case counts as a change) or a new one without `@since`. `cyx check` prints
  the oldest Codeyard an extension needs, worked out from the functions and types its component imports; a component
  built against a newer 1.x runs when every item it uses is there, and one that uses an item this Codeyard lacks gets
  `CY0701`. Components built against API 0.1 keep running (the exthost renames their imports to 1.0.0 before
  compiling; the shape is the same), with a warning from `cyx check`; `cyx publish` and Yard refuse them. The API
  review before the freeze kept the WIT's shape: its docs now say what a `denied` permission and an instance's
  `remote.account` hold (#183, #184), and `code-action.edits` names its `uri`. In the Rust SDK `ctx.state().remove`
  is now `delete`, and `keys` is new. Migration guide: docs/extensions/migrating-to-1.0.md.
- The Pull Requests view's command is **Pull Requests: Show…** (was "GitHub: Pull Requests…"), since it lists every
  provider's requests; the palette still finds it by its old name and by "merge requests" (#186). When no remote is
  claimed, the message names the providers there are.
- An `auth.fetch` to a host outside the account's is refused with `[[oauth]] <id> hosts = ["<host>"]`, and both SDKs,
  both mock hosts, `cyx test` and the Inspector's **Copy fix** now say to add the host to that `[[oauth]]` table instead
  of the `auth` line the manifest already has (#183). The SDKs fill in a manifest line only when Codeyard names just
  a permission (`net`); any other text it sends is kept.
- `cyx package` leaves the `test/` and `tests/` folders at the top of an extension out of the package (#187).
- Deleting in the Explorer on an SSH host says it is permanent: one entry asks "Permanently delete “name” on
  host?", a selection "Permanently delete N items on host?", both say the host has no Trash, and the button is
  "Delete Permanently" (a critical alert, Escape cancels). File questions name the connection's host. Bulk copy,
  move and delete over SSH are covered by tests against a real `sshd`, including a name already taken in the
  destination and a file that cannot be removed.
- Source Control's change list is fully accessible: each file is a row with its change, its place in the group and its
  own Stage/Unstage (Mark as resolved) and Discard buttons (Discard asks first, as the painted button does), the commit
  button says why it is disabled, and a large status exposes a scrollable range of 500 lines instead of every row. The
  list has a keyboard cursor (↑/↓/Home/End, ↵ opens the diff, Space stages or unstages, ←/→ fold and unfold groups,
  Delete or ⌘⌫ discards) that screen readers see as the focus. Actions from an old tree do nothing once the file has
  changed state or another repository is shown.
- Right Arrow in the Explorer is now covered by tests for open folders that are empty, still loading, unreadable or
  have children, and for a file, both for the selection and the accessibility focus (no behavior change: it enters a
  child only when one is shown). Tests also cover the Rust command notices for a restricted workspace and an unavailable
  language server.
- Source Control builds only the change rows on screen, so a repository with thousands of changes (generated files,
  a large merge) scrolls and refreshes without delay. The list keeps its scroll position and focus when the status
  refreshes.
- Screen readers reach every row of a large Explorer and every line of a long file: instead of the first 5,000 rows
  and a window around the caret, the accessibility tree shows a range centred on the selection or caret, or on the
  visible area after a scroll. Scroll up/down by a page moves it, and row positions and text offsets count from the
  start of the tree or file.
- Deleting a file or folder deletes it permanently after asking, instead of moving it to the Trash:
  `files.enableTrash` is now off by default. Turn it on to keep using the Trash for local files.
- The settings and shortcuts import is "Preferences: Import Settings and Shortcuts from Another Editor", and its
  messages no longer name another product. A shortcut block written by an earlier import is still replaced by a new one.
- Installed extensions with code start no process when Codeyard opens: the check that the system sandbox works now
  runs when the first one starts. The startup measurement with 100 installed extensions now has 50 with code.

- A task no longer types its command into a new terminal: it runs as a process on the folder's machine, so a value from
  a variable can never become more shell code, and Stop ends the processes it started.
- The Linux agent's size budget is 1664 KB (was 1600 KB): running tasks as processes on the server needs its own
  request in the protocol.
- In a workspace file with several folders, trust is asked once for the workspace: "Do you trust the authors of this
  workspace?" names the workspace file, and the answer (or "Workspace: Trust This Folder" while the workspace is open)
  trusts the file's folder and every folder it lists, on that host only. Before, the file's folder and each folder were
  asked about one by one.

### Fixed

- `scripts/perf/run.sh` measured nothing when another window covered its window: the measured app is never activated
  (it must not take the keyboard), so its window opened behind the front app's windows, and macOS draws nothing in a
  covered window. With `CODEYARD_PERF_ON_TOP` the window stays above normal windows and on every Space, without
  activating the app. A build that has a scenario's hook (the baseline too) must now give all of its samples and every
  memory scene a value, or the run stops with a message; before, a baseline's missing samples were left out of the
  comparison without a word. A baseline runs with its own `codeyard-exthost`: the candidate's answered none of the
  older app's provider requests.

- Extension authoring, second outside-author trial:
  - In an untrusted folder a refused write, `net` or other sending call says the folder is untrusted, not that the user
    turned the permission off, and the fix line is the right one.
  - `cyx package` finds a private key only as a whole PEM, PGP or PuTTY block, never in `extension.wasm`,
    `extension.js.map` or a README, LICENSE, CHANGELOG or `.md` file, so extensions that talk about keys package.
    It now also leaves out `.ppk`, `.p8`, `secret.*` and `secrets.*` files, and stops on an obvious GitHub, AWS or
    Slack token in a text file, naming the file and `.cyxignore` as the way to leave it out.
  - The publish workflow `cyx new` writes uses an action that installs the `wasm32-wasip2` target (Rust) and Node.js
    (TypeScript); until a release can publish API 1.1 code it follows `main`, with a comment saying when to pin it.
  - A user's setting is `ext.<id>.<key>` in settings.jsonc; the reference said `<id>.<key>`. Docs say API 1.1, and
    that **Allow Host** needs the packaged install. `cyx check` reports API 1.1 for a manifest with `net-from-setting`.
  - `codeyard --accept-permissions` without `--install-extension` is refused with a message, not ignored.
  - `cyx test` tells a repeated refusal once. TypeScript: `setTimeout` and `setInterval` throw and point at
    `ctx.every` instead of hanging, and a 20 MiB answer fails as `invalid` like it does in Rust.
  - The mock hosts and `cyx test` judge `core:editor.open` as Codeyard does (argument, URI, `read` on the file) and find
    fake files by the resolved path (`./`, `..`, `//`, `%2e`).
- Typing in a large file with every region folded took about twice as long as with nothing folded (22 ms against
  11 ms a key in a 50 000-line file): the folded lines were worked out again from the fold marks several times per
  key, and the fold ends were found by looking up every line anew. The folded lines are now kept until the text or
  the folds change and the fold ends come from one pass over the lines (15 ms a key; moving down 10 ms → 4 ms).

- A command or view answer over 8 MiB fails its request with `invalid: the result is 20 MiB; at most 8 MiB`; it was
  dropped, the request waited until its deadline and the extension was counted as crashed.
- `cyx test` no longer passes a command that throws, has no handler, or returns any error: an error is an answer only
  after a call it cannot serve offline (a missing fixture, file or program). An `async activate` that rejects fails it.
- The mocks check `open-external` for a user action as Codeyard does, and both mocks' and `cyx test`'s `find-files`
  list only what `read` allows.
- A native `cargo build` or `cargo clippy` of a Rust extension no longer warns that its type is never constructed.
- The branch-status example declares its status bar item, which Codeyard did not show.

- `codeyard --update-extensions` printed nothing and exited 0 when `extensions.lock.jsonc` could not be read (an
  unknown key after a hand edit), hiding registry errors and revocations. It now prints why and exits 1, and a
  version a registry revoked is still stopped: revocations are checked against the packages in the store, without
  the lock.
- A revoked version whose newer version was withdrawn stayed stopped for good. It now goes back to the newest earlier
  version that may install, as a withdrawn one does, and `--update-extensions` says so when there is none.
- A denied read or write outside the workspace, and a `fetch` to an IP address, told the author to add a line no
  manifest accepts. Both now say they cannot be granted, and why (Rust and TypeScript SDKs, the Inspector).
- Typing words at the top of a large file no longer redraws the window behind the keys (#215). A key whose reparse
  goes to the background (stray words make tree-sitter recover from the error across the whole file) used to redraw
  the editor when that parse finished, often just as the next key came. Now the new tree is compared with the old
  one only where they differ (`changed_ranges`) and only on the lines drawn; it asks for a frame only if something on
  screen changes color, and only once typing pauses, since the next key's frame draws it anyway. Edits made while a
  parse runs go to the next background parse instead of being reparsed on the UI thread, and the old tree (about
  3 ms to free for a 10 000-line file in that state) is freed on a thread of its own. Painting applies any edit the
  tree has not seen yet, whichever way it reached the buffer.

- Extension authoring, from two outside-author trials: `cyx check` names the Codeyard release an extension needs
  without claiming that this preview build cannot run it, and names the API in "this Codeyard (API 1.0.0)"; a
  component built against an old API reports it once, not once per import. `cyx publish` says "in 48 hours", not 49,
  and a pre-release of code installs at once for those who opted in (the registry said it waited). `cyx check`
  refuses key bindings with unknown modifiers or keys (`hyper-banana`), warns about a key a built-in shortcut uses and
  about a language id that is neither built in nor added by the extension (with the likely one: `rustt` → `rust`),
  and reports every unknown manifest field at once. `cyx check --fix` lists every missing permission it cannot add,
  and for TypeScript says what to add by hand. `cyx package -o` into a missing folder fails before the build. A list
  or table with too many rows says to show fewer, not to "send at most 10000 at a time".

- `cyx dev` notes the sources before its first build, so a file saved while it builds, checks the extension or
  starts Codeyard is rebuilt on the next poll instead of being missed until the next save.
- Unit tests no longer start an installed language server from generic editor and workbench fixtures: under test
  `lsp.enabled` defaults to off and LSP integration tests turn it on explicitly. The shipped default stays on.
- On a busy machine, closing a window could leave its terminals running for 5 minutes, and the last keys typed into a
  terminal before quitting could be lost: the connection to the terminal daemon was closed before the window's `Bye`
  (or those keys) had been written. The connection now closes only after everything queued has gone out (at most 5
  seconds for a daemon that stopped reading).
- Views made from `cyx new view` (Rust and TypeScript) and the `hello-view` examples drew an error instead of their
  content: they wrote a `{ "type": "column", … }` shorthand that Codeyard refuses ("unknown field `type`"), while
  `cyx test` said "view main draws" (#182). Both SDKs now have `ui` functions that write the documented form
  (`{"key", "element": {"<kind>": {…}}, "children"}`), the templates, examples and docs use them, and in TypeScript
  `ViewNode` types the tree element by element (`Element`, which typed the shorthand, is now one element). The mock
  hosts' `render` and `cyx test` check a tree with Codeyard's own rules (the TypeScript mock with a copy generated
  from them and tested against the same cases), and `view_event` / `viewEvent` refuse an event Codeyard never sends, such as `{"click": "add"}`.
- Zen mode no longer leaves a view of the secondary side bar (such as the Explorer) in the accessibility tree, or its
  refreshes running, while it is not drawn; actions from an older tree on a hidden Explorer do nothing.
- `cyx package` no longer puts the `.cyx` it wrote last time into the new package (it doubled in size on every run).
  New extensions from `cyx new` ignore `*.cyx` in git.
- `cyx check` on a code extension that is not built yet says to run `cyx package` or `cyx test`, which build it.
- Typing in a large file could wait 60 ms or more per key for the syntax tree: an edit that makes tree-sitter recover
  from an error across the whole file (stray words or a half-typed `->` between the items of a 10 000-line Rust file)
  reparsed all of it before the key was drawn. A key now reparses for at most 4 ms; a longer reparse finishes in the
  background and the colors catch up when it is done. This was most of what `typing.keystroke_frame_completion_ms`
  measured (median 78 ms, the completion provider itself answers within a millisecond); it is now close to plain
  typing.
- A debug configuration with `"stopOnEntry": true` run with lldb-dap said "Paused on exception: signal SIGSTOP": the
  adapter reports the stop on entry as a signal. The first stop of such a session now reads "Paused on entry"; a
  SIGSTOP later on is still reported as an exception.
- A task stopped with "Tasks: Terminate Task" was reported as an error ("Task 'watch': cancelled"). A stop the user
  asked for is reported as information now; a failed task is still an error.
- A workspace file's debug configurations understand `${workspaceFolder:name}` and `${workspaceFolderBasename:name}`, as
  its tasks already did. Before, the program path kept the variable as written and debugging failed to start with
  "Expected process to be stopped".
- After "File: Open Active File as Workspace" (the way to open a workspace file on an SSH server), no shortcut worked
  until something was clicked: the tab that had the keyboard closed with the old folder. The keyboard now stays in the
  window whenever what had it goes away.
- Over SSH, the terminals of a workspace file with several folders were not taken back after a restart: their session was
  kept under the first folder while the next start reopened the workspace file. It is kept under the workspace file now.
- A folder deleted while it was open in the file tree no longer stays there with "not found" beside it. When a
  folder's read finds it gone, the tree drops it and reads its parent again, so this also covers folders deleted
  outside Codeyard, where the watcher may report only the folder itself.
- Quick Open in a workspace with several folders finds a file by its folder's name and path, as in `repoB/src/lib.rs`,
  for every folder. Before, typing a folder's name found nothing, so files with the same relative path in two folders
  could not be told apart.
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

- A symbolic link in the workspace no longer takes an extension outside what it may read or write. With
  `read = ["src/**"]`, `src/out -> /etc` gave it `/etc/passwd`, and `src/settings.txt -> ../.env` gave it `.env`
  without naming it. Every file call (read, stat, write, delete, rename, new folder), file listing, search result
  and `core:editor.open` is now checked again where the path really leads on the workspace's host, local or SSH
  (protocol 18, `RealPaths`): it must stay in the workspace and match the rules; listings and search leave such files
  out.
- `codeyard --install-extension publisher.name` installed from a registry whatever the version asked for (all files
  and the network, or programs) without showing it. A registry version or package file that asks for permissions now
  prints its consent sheet and installs only with `--accept-permissions`, for both kinds of install.
- Diagnostics an extension pushes need a `[[diagnostics]]` declaration and a file of a language it lists, in the
  workspace or open in an editor. A message with bidirectional or zero-width characters is dropped, one longer than
  4096 characters is cut short, and one extension has diagnostics on at most 1000 files at once.
- The terminal daemon (`codeyard --daemon`) stops on SIGTERM. Started from a background thread, it inherited a signal
  mask that blocked SIGTERM on macOS, so only `kill -9` stopped it.
- A profile with its own `CODEYARD_STATE_DIR` keeps extension accounts and `secrets` apart from the user's in the
  system keychain, so a test run neither reads nor replaces them.
- `cyx package` and `cyx publish` no longer put secrets and build input in a package: `.env` files, keys (`*.key`,
  `*.pem`, `*.p12`, SSH keys), `.npmrc` and other token files, hidden files, `src/`, Rust and TypeScript sources,
  `Cargo.toml`, `Cargo.lock`, `package.json` (which held the author's local `--sdk` path) and lock files stay out.
  `cyx` names each secret it left out, and a file holding a private key stops packaging whatever its name.
  `.cyxignore` leaves out more, and `cyx package --dry-run` lists what a package would hold. The developer guide says
  what goes into a package.
- `minimatch` 11.0.1 in the TypeScript build tools and the extension host benchmark guest
  (GHSA-g3hv-75wg-m249): a pattern with many brace and glob parts could take unbounded time to expand.
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
