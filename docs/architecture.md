# Architecture

Codeyard is a native code editor written in Rust on GPUI. Design goals: low memory, fast start, one code path
for local and SSH workspaces, no telemetry, no extension host, no embedded web view.

## Workspace layout

```
crates/
  buffer/   rope buffer, edits, undo, cursors and selections; no UI dependency
  proto/    agent protocol: request/response types, frame codec, VERSION
  agent/    remote binary: file system, search, watch, pty, child processes, big-file index, daemon
  host/     `Host` trait: LocalHost (agent in a thread) and RemoteHost (system ssh); askpass bridge,
            file transfer, ~/.ssh/config editing
  syntax/   tree-sitter grammars and highlight queries
  schema/   JSON/YAML schema engine (parse with spans, validate, hover, completion, SchemaStore)
  spell/    Hunspell dictionaries, tokenizer rules, personal dictionary, dictionary download
  state/    local SQLite state (`schema.sql`)
  theme/    colour tokens -> Rust constants (`design/tokens*.json`, `build.rs`), built-in themes
  app/      GPUI views, commands, keybindings, the workbench; the only crate that depends on GPUI
vendor/
  gpui/     GPUI 0.2.2 with local patches (see vendor/gpui/PATCHES.md)
  xattr/    local patch so the vendored GPUI compiles on Linux
```

Rule: nothing except `app` depends on GPUI. `agent` also has no UI or network dependency; it is a static
musl binary of about 1.5 MB.

## The workbench

`crates/app/src/workbench.rs` owns the `Workbench` entity. To keep it reviewable it follows three rules:

1. **Topic files.** Behaviour lives in `workbench_<topic>.rs` files (`workbench_git`, `workbench_hosts`,
   `workbench_http`, `workbench_schema`, `workbench_spell`, `workbench_pulls`, `workbench_transfer`, …). They are
   child modules included with `#[path]`, so they read the workbench's private fields with `use super::*`.
   The editor is split the same way (`editor_*.rs`).
2. **State groups.** New state goes into a group struct in `workbench_state.rs` (`TabsState`, `LspState`,
   `GitState`, `ChromeLayout`, `NavState`, `ConfigState`, `PersistState`, `RemoteState`, `RootsState`,
   `TaskState`, `DebugUi`), not into `Workbench` itself. `LspState` also carries the schema, spell and `.http`
   state. The `Workbench` struct stays under 80 lines.
3. **Large enums** (`OverlayKind`, `InputPurpose`, `MenuCmd`) live in `workbench_types.rs`.

Compare Folders is three files: `compare_logic.rs` (matching two `HashTree` scans, plans, report; no UI), `compare_folders.rs` (the tab) and `workbench_compare.rs` (picker, trust gate, copies and deletes checked against the scan).

`scripts/arch-check.sh` enforces size limits and only ever lowers them: `workbench.rs` ≤ 2750 lines,
`editor.rs` ≤ 1800, every other source file ≤ 2500 (test files ≤ 6000), `Workbench` struct ≤ 80 lines. When a
limit is hit, split the file instead of raising the limit.

Test-only accessors (`*_for_test`) are in `workbench_testing.rs` and the editor's own files; tests never reach
into private state any other way.

## Stack

| Layer | Choice | Why |
| --- | --- | --- |
| Language | Rust 2024, cargo workspace | no GC, predictable memory, one binary |
| UI | GPUI 0.2.2, pinned with `=`, vendored | in an early comparison with Iced (throwaway programs, no longer in the repository) it used about a third of the memory and started 7× faster; API is pre-1.0, so upgrades are deliberate |
| Text | `ropey` | O(log n) edits, constant cost on large files |
| Syntax | `tree-sitter` with highlight queries | incremental, only the visible range is coloured; only the front tab keeps a tree |
| Fuzzy matching | `nucleo-matcher` | quick open and palettes |
| Search | `ignore` + `regex` in the agent | respects `.gitignore`, streams results, cancellable; the same matcher serves find-in-file (Turkish i/ı/İ/I folding) |
| Structural search | `tree-sitter` in the app; the agent's text search picks the files | App-side on purpose: the agent has no tree-sitter and stays a 1.5 MB binary, and no protocol request is added. The agent finds files that contain the pattern's longest token, the app reads only those and parses them one at a time |
| Terminal | `alacritty_terminal`; the agent opens the pty | mature VT emulation, same path locally and over SSH |
| SSH | the system `ssh` with ControlMaster | `~/.ssh/config`, ProxyJump, agents, 2FA and certificates come for free |
| Protocol | length-prefixed `postcard` frames | small and fast; file contents travel as raw bytes |
| Git | the system `git`, run through `Host::exec` | hooks and signing behave exactly as in a shell; parsing lives in the app |
| LSP / DAP | own JSON-RPC clients over `Host::proc_*` | the server runs where the files are |
| File watching | agent polls a small path set once a second | no dependency, identical on NFS and every other file system |
| Encodings | small built-in tables (UTF-16 with BOM, Windows-1254, ISO-8859-9/1) | `encoding_rs` would add about 1 MB |
| Settings | JSONC, comments preserved on write | `~/.config/codeyard/settings.jsonc`, `keybindings.jsonc`, `themes/`, `.codeyard/settings.jsonc` per folder |
| Local state | SQLite (`rusqlite`, bundled), WAL | atomic and crash safe; see `crates/state/schema.sql` |
| Allocator | `mimalloc` in the app, system allocator in the agent | lower fragmentation; small agent |
| Schemas | `jsonschema` (no default features), `yaml-rust2` | validation and hover without Node; alias budget bounds YAML expansion |
| Spelling | `spellbook` (pure Rust Hunspell) | English built in, other languages installable |

No server, no account, no plugin store. Outbound requests: SchemaStore and schema downloads (cached 7 days, can be disabled),
dictionary installs, GitHub pull request listing, requests sent from `.http` files and update checks. The update check is the one
the app starts by itself, and only when the user chose daily checks (`update.check`); otherwise it happens when the user asks
for one. It downloads two small files from github.com (see [Updates](#updates) for exactly what GitHub sees).

## Updates

`update.rs` (check, download, verification) and `update_install.rs` (installing) hold everything except the UI.

- **What is fetched.** From `<repository>/releases/latest/download/` (`CODEYARD_UPDATE_URL` replaces it): `SHA256SUMS.txt` and
  `SHA256SUMS.txt.sig` (at most 64 KiB and exactly 64 bytes). Only when the signed list names a newer version: `RELEASE_NOTES.md`
  (256 KiB) and, when the user agrees, the installer for this platform (400 MiB); these two come from
  `<repository>/releases/download/v<version>/` (the tag of the version the verified list names), so a release published in between
  cannot be mixed with the list. No GitHub API, no token. github.com answers each request with a redirect to
  `release-assets.githubusercontent.com`; what GitHub sees is the IP address and `curl`'s user agent. No cookies, no identifier in
  the URL, nothing about the user's files. The requests are made by the system `curl` (absolute path on macOS; https only, TLS 1.2+,
  five redirects, timeouts, stall detection) and a `404`/`410` is told apart from a connection failure by the status in curl's message
  (curl 8 on macOS ends an HTTP/2 error with exit status 56, not 22). `SHA256SUMS.txt` answering 404 means there is no release to look
  at (`NoRelease`: not an error to retry, not "up to date" either, because the latest release cannot be judged); `SHA256SUMS.txt.sig`
  answering 404 means the release is unsigned (`Unsigned`, like v0.1.0 today). A signed list that names no installer for this
  platform is still an update (`asset: None`: the UI points to the release page); one that names no installer for any platform, or no
  `RELEASE_NOTES.md`, is refused. The list's format is strict and `scripts/verify-release.sh` applies the same rules.
- **What is verified, in this order.** The Ed25519 signature over the exact bytes of `SHA256SUMS.txt` against the public keys
  built into the app (`TRUSTED_KEYS`, a list so that a key can be rotated); only then is the list parsed. The version comes
  from the signed asset names (`x.y.z`, digits only) and is offered only if it is strictly newer than the running one and not
  lower than the highest version this installation was ever offered (`update.highest_seen`, a replayed older list is logged and
  ignored). The notes and the installer must match their signed SHA-256; the installer is hashed again right before it is mounted
  or run.
- **What the signature cannot do.** It has no date and no revocation. Anyone who can answer a client's requests can freeze it on an
  older signed release, and a client that never saw a newer one cannot recognise a replay (`highest_seen` only helps clients that
  did). The remedy is a key rotation in a normal release (see `TRUSTED_KEYS` and [SECURITY.md](../SECURITY.md#updates-and-signatures)).
- **When.** `Policy::Daily` (setting `update.check`) allows one successful check in 24 hours; a failed check does not count, so
  the app looks again every three hours while it runs, at most three failed attempts per rolling day (`retry_allowed`, kept in
  memory: a restart starts fresh). `Ask` and `Manual` never check by themselves. A check never installs anything: download and
  install are separate steps that need the user's confirmation. The first-run question waits for a quiet keyboard (3 s without a key).
- **Installing.** macOS, app bundle in a writable folder: the disk image is mounted read-only at a private mount point, the app
  is copied next to the installed one, then checked (bundle identifier, version, `codesign --verify --deep --strict`), then
  exchanged with it in one atomic step (`renamex_np` with `RENAME_SWAP`), or, where the file system cannot, swapped by two renames
  that are undone on failure; the swapped-out copy is deleted. Nothing clears the quarantine flag: `curl` does not set one, so an
  update installed this way skips Gatekeeper's first-launch check (builds are not notarized yet). Leftovers of a run that died
  (`.Codeyard.update-<pid>.app`, `.Codeyard.old-<pid>.app` beside the app, `codeyard-update-mnt-<pid>-*` mount points, an `updates/`
  download folder older than two hours) are removed before staging and at start, never those of a live process, and never the backup
  while the installed app is missing. Windows, Inno installation: the verified setup program runs silently. Everything else (debug
  builds, bare binaries, Linux, a portable zip, a read-only `/Applications`) is `Manual(reason)`: the UI shows the release page and
  says which reason it is. Helper programs are run by absolute path (`/usr/bin/curl`, `hdiutil`, …); tests inject a different `curl`
  through `Config`.
- **The UI.** `workbench_update.rs` is the state machine (`Idle`, `Checking`, `UpToDate`, `Available`, `Downloading`, `Installing`,
  `Ready`, `Error`) and everything the dialog says; `workbench_view_update.rs` draws it and the status bar item. The network, the
  installer, the clock and quitting are behind `UpdateBackend` (`update_backend.rs`), so tests script every outcome. Machine-written
  values (time of the last good check, skipped version, installed version, the notified version, when the question may come back) are
  keys `update.*` in the state database; the one setting, `update.check`, is user-only like `ui.language`. Restarting goes through the
  normal quit path (hot exit), starts the new version first and quits second.
  **The restart hand-over** (`update_restart.rs`): after hot exit has saved every window and before the relaunch, the old process writes
  `update-restart.json` into the state folder (atomically): the version just installed, the time, and for each window with a folder its
  local path or SSH alias + path, plus which window was in front. The relaunch helper starts the app with `--after-update`
  (macOS: `open -n <app> --args --after-update`, the arguments are positional parameters of the script, never script text; Windows: the
  `[Run]` entry of `codeyard.iss`, unverified there); only with that flag does `main` read the file, and it deletes it first: a second
  start finds nothing. It is ignored (logged, ordinary start) when it is missing, unreadable, not valid, from another version, older than
  ten minutes or from the future, or when a folder or `--connect` is on the command line. It is never read from a project and carries no
  tabs: a window's tabs and unsaved edits come from the per-folder session when `set_root` opens the folder
  (`window.restoreSession`, `files.hotExit`). If the relaunch fails the file is removed again. Why a file and not folder arguments: several
  windows' paths would have to pass through `open --args` and the installer's parameters, with their quoting; and this way the
  consumer can check age and version. While ANY window is swapping the app in, quitting asks in
  every window and that window cannot be closed (its result is the only thing that records the installed version). Closing the dialog
  of a failed update drops the failure, so its status item goes and the next background check is not held back. User-facing
  description: [user guide](user-guide.md#updates).

## Local state

`crates/state/schema.sql` is the single source of the SQLite schema (`hosts`, `workspaces`, `open_editors`,
`file_state`, `backups`, `history`). One user, one process; access control is file permissions. A workspace is
unique per host + path (`coalesce(host_id, 0)` index). Settings and keybindings are not in the database; they are
hand-editable JSONC files.

Safety rules of the state crate: opening runs the first-time schema and every migration in one `IMMEDIATE` transaction
(two windows starting together cannot collide, an interrupted setup leaves nothing half-made); a database written by a
newer Codeyard is refused instead of used; writers use `IMMEDIATE` transactions and a 5 s busy timeout, WAL with
`synchronous = NORMAL`; "remove from recent" only *hides* a workspace (`hidden = 1`), so its unsaved-work backups, tabs and
favourites survive and return when the folder is opened again.

Files the user edits by hand (settings, keybindings, projects, themes) and the hot-exit blobs are written with
`fsutil::write_atomic` (exclusive temp file, `fsync`, `rename`; symlinks and permissions are preserved), never with a
truncating `fs::write`.

## Design decisions that bite

- **No round trip per key press.** Editing happens in the local rope; the network is used for reads, saves,
  searches and process I/O.
- **Save conflicts.** `Write` carries the expected hash. If the disk changed, the agent answers `Conflict` and the
  UI offers overwrite / compare / reload. A dirty buffer is never reloaded automatically.
- **Atomic saves** keep owner, mode and symlink target (the real file is written, via temp file and rename).
- **Large files.** Above 1 MB syntax highlighting is off. Files over 32 MB (`proto::MAX_FILE`) are not loaded into
  a rope: they open in a read-only chunked view backed by the agent's incremental line index, which is also what
  log follow uses. An editable file lives completely in memory; that is a deliberate limit.
- **Multiple cursors.** The buffer keeps a sorted, non-overlapping list of selections plus a primary index. One edit
  group is one undo step (parts apply back to front so coordinates stay valid; undo walks front to back).
- **Unicode.** Cursor movement is by grapheme cluster; LSP positions are UTF-16; CRLF, BOM and the file's own
  encoding are preserved on save.
- **LSP sync.** Incremental `didChange`; results carry an epoch so late answers for another document or a dead
  server are dropped. Whole-document requests are skipped on very large files.
- **Untrusted workspaces.** See "Workspace Trust, in one place" below: nothing in a folder runs, loads or downloads
  anything beyond the table's allowances until the user trusts it.
- **GPUI upgrades** are deliberate: the version is pinned, local patches are listed in `vendor/gpui/PATCHES.md`
  and re-applied on every upgrade.

## Opening files from the system

Finder's *Open With*, dropping a file or folder on the Dock icon and `open -a Codeyard x` arrive as `file://` URLs through
GPUI's `on_open_urls`; `sysopen.rs` turns them into paths and dispatches: a file becomes a tab of the active window and
never changes the open project (without a project the file's folder opens, as on the command line); a folder activates the
window that already has it, otherwise opens in the empty active window or a new one. The Info.plist declares text, source
and folder document types so macOS offers Codeyard.

## Workspace Trust, in one place

An untrusted folder (anything not opened before the trust gate existed and not trusted since) is restricted like this:

| What | Untrusted | Trusted |
| --- | --- | --- |
| tasks, tests, debugger | ask first (“Do you trust the authors of this folder?”) | run |
| project `terminal.shell`, `lsp.servers`, `json.schemas`, `yaml.schemas` | ignored (the status bar names them) | applied |
| language servers | **not started**; one notice per folder says how to turn them on; trusting starts them at once | start on opening a file |
| `$schema` URLs and schema `$ref`s | downloaded only from SchemaStore (cached copies still work); a network-share path (`//host/share`, `file://host/…`) is never opened, since Windows would log in to that host | any `http(s)` address |
| `git status` (every 4 s) | `core.fsmonitor` turned off | as in a terminal |
| `git show` / `blame` | `--no-textconv` always (raw bytes are also what hunk staging needs) | same |

`security.workspaceTrust.enabled` (user settings only) turns the whole gate off. Folders recorded in the recent list when the
language-server gate was introduced are trusted once (`trust_migrated_v1`) so existing projects keep working.

## Known limitations

Deliberate, with the reason:

- **Interface language is bounded.** `i18n.rs` looks up English text in a table compiled in from `crates/app/locales/<code>.tsv` (parsed
  once on first use, about 500 entries, no dependency; English costs nothing). Only the application menus, palette labels, Welcome
  screen, status bar and Git confirmations go through it (user guide, *Interface language*); everything else is English on purpose, a
  missing key falls back to English, and command ids, settings keys and test accessors never change.
- **Event queues between the connection reader and the UI, one by one.** Search results are bounded by construction:
  `search.maxResults` (at most 100 000) is enforced by the agent with an atomic reservation across its worker threads, so a
  slow client queues at most that many matches. Terminal output is capped at 32 MB of unparsed bytes (the excess is dropped with
  a visible note; the exit event is never dropped). File-change reports and language-server diagnostics/progress are *folded*
  while the UI is busy (`ui::bridge_coalescing`: one entry per watched path, one diagnostics list per file, one progress line),
  which loses nothing because a later report replaces an earlier one. **Process output (language servers, debug adapters) is
  still an unbounded queue in memory**, on purpose: those messages must never be dropped, and a bounded queue would have to
  block the connection reader, which is shared by every call, terminal, search and the heartbeat (two missed pings tear the
  connection down). Real back-pressure needs per-stream credit in the protocol (the client acknowledges bytes, the agent stops
  reading the child's pipe when the window is used up), which means a protocol version bump; until then the bound is
  "whatever a server writes while the UI is stalled", and the other LSP/DAP events (messages, questions, log lines, debug
  output) are queued in order without folding.
- **Diff drivers.** The editor never runs `git diff` itself (it diffs with `similar`), so external diff programs are not
  reached; `git log --follow` and `--name-only` run no drivers.
- Remote `.http` requests and `$schema` downloads in a trusted folder ask once per new host and session (`http.confirmRequests`,
  default `remote`; `never` restores the old silence, `always` confirms every `.http` request). The host is the one the request
  starts with. Redirects are not followed (no `curl -L`), so no unapproved host receives the request. Headers of an `.http` request
  (an `Authorization` token, say) are curl arguments and so visible in `ps` to other users of the machine it runs on; on a shared
  server keep secrets out of the request file or send from your own computer (`http.runOn`: `local`).
- Windows passes its native check script but has not been verified interactively on a physical machine (SSH login, IME, mixed
  DPI, installer); macOS builds are ad-hoc signed (notarization script untested); no screen-reader testing (see
  `platforms.md`, `accessibility.md`).

## Budgets and gates

Measured by `scripts/memcheck.sh` (macOS `footprint`, 1280×800 @2x; the screen buffers are part of the number):

| Scenario | Budget |
| --- | --- |
| terminal open | 85 MB |
| empty window (welcome screen) | 85 MB |
| small Rust file (360 KB, in a repo) | 125 MB |
| 10k lines, plain text | 95 MB |
| 10k lines, syntax highlighting | 130 MB |
| remote agent daemon, idle | 8 MB |
| app binary | 60 MB |
| agent binary (Linux musl) | 1600 KB |

Before a release all of these must pass: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, `cargo deny check` (licences, advisories, sources), `scripts/arch-check.sh`, `cargo test --workspace`,
`scripts/memcheck.sh`. A new feature that adds 10 MB or more is investigated first; a budget is only raised as a
last resort and the reason is written next to it in the script.

## Dependencies

Direct dependencies are kept at their latest stable version; `Cargo.lock` is committed. `cargo-deny` rejects
unknown licences, known vulnerabilities, yanked crates and wildcard versions, so path dependencies carry an explicit
`version = "x.y.z"` that is bumped with every release.
