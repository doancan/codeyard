# Codeyard extension platform

**Status:** accepted 2026-10-28. Work is tracked in the extension platform epic on the Codeyard project. The current
release has no extension support yet; this document describes what is being built and in which order.

The design is deliberately small enough for a small team to build and run in its first year. Anything that did not
earn its place is in [Later](#13-later) with a one-line reason.

This is the maintainer document. Authors read [docs/extensions/](extensions/developer-guide.md), which never
mentions §3, §6.2, §8, §10 or §11.

## 1. Summary

The model is VS Code's: one manifest, contribution points, activation events, a typed API, a scaffold, a
debug loop and a publish command. That model produced a large ecosystem, so we copy it. We fix only the four
problems that hurt VS Code users.

| VS Code problem | Codeyard answer |
| --- | --- |
| Every extension can do anything the user can do | Code runs as WebAssembly in a sandboxed process and can reach only what its manifest declares. The app checks every call |
| Eager activation slows startup | Activation is inferred from contributions and there is no `*`. 100 installed extensions cost under 5 ms at startup and start no process |
| Webviews are heavy, inconsistent and an XSS surface | Extension UI is a tree of built-in elements drawn by GPUI with the user's theme and accessibility support |
| Supply-chain attacks through stolen tokens and keys | Keyless publishing from CI, a registry-signed index, a re-prompt when permissions grow, a 48 h wait before new code reaches users, and revocation within 24 h |

We keep VS Code's contribution names and its colour theme and snippet formats, so existing themes and
snippets work unchanged.

Authors need five concepts: **manifest, contributions, activation, permissions, publish.** Everything else
(`when` expressions, untrusted folders, limits, storage) is reference material and is not needed for a first
extension.

## 2. Principles

- **Declarative first.** Themes, snippets, keymaps, languages, schemas and settings need no code.
- **Undeclared means denied.** An extension can do only what its manifest lists and the user allowed. The app
  checks every call and never trusts the extension process.
- **No third-party code or JIT in the app process.** wasmtime and Cranelift live only in `codeyard-exthost`.
  `scripts/arch-check.sh` fails if `cargo tree -p app -e normal` contains `wasmtime`, `cranelift` or
  `ext-host`.
- **Zero cost when inactive.** An inactive extension starts no process, makes no request, causes no wakeups.
- **Local and SSH are the same.** Wasm always runs on the UI machine. Files, processes and git go through the
  window's `Host`, exactly as the core does.
- **No telemetry.** The installed list, search queries and usage never leave the machine.

## 3. How it works

```
 install                      activate                      run                          update
 ───────                      ────────                      ───                          ──────
 cyx / Market                 first matching event          exthost ──call──▶ app        daily: signed index
   │ verify signatures          │ (command, view,            (Wasm,          broker        │ same permissions:
   │ show consent sheet         │  language, file, …)         sandboxed)     │             │   install after 48 h
   │ unpack to store            ▼                              ◀──reply──  Host::*         │ more permissions:
   │ register contributions   spawn exthost for this ext                   (Local|SSH)     │   card shows the diff
   ▼ (no process yet)         load cached compiled code                                    ▼
 compile .wasm in background  call activate()                                            revoked: stopped at once
```

Processes:
- **`codeyard` (app).** Holds the contribution index, the broker and every permission decision. Never runs Wasm.
- **`codeyard-exthost`.** One process per **active** code extension, running wasmtime inside an OS sandbox: no
  network, no exec, read access only to that extension's package and data directory. It exits 5 minutes after
  its last call or timer. An escape gets one extension's permissions, and crash blame needs no logic.
- **`codeyard-exthost --compile`.** Short-lived. Reads one `.wasm` on stdin, writes compiled code to stdout.
  The app stores it as `<wasm-sha>.cwasm` with a `.sha256` sidecar; the exthost checks the hash before loading,
  so one extension cannot plant compiled code for another.

Resource limits, all from one `limits` table in `ext-manifest` (author docs and perf rows are generated from it):
- wasmtime epoch interruption makes guest code yield every 10 ms, so a busy task never blocks providers;
- a provider reply that misses its deadline is dropped, not punished;
- `StoreLimits` caps each instance's linear memory; an OS cap (Job Object, `RLIMIT_AS`) caps the process;
- going over a memory cap, the IPC frame cap or the IPC byte rate kills the process.

The app and exthost must share a build id. When the app updates in place, running exthosts are stopped and
respawn on the next event.

Sandboxes:
- **macOS:** Seatbelt (`sandbox_init`).
- **Windows:** AppContainer (LPAC) without `internetClient`, plus a Job Object. A Low-IL token alone still
  reads the user's files, so it is not enough.
- **Linux:** landlock, seccomp and an unshared network namespace.
- Installed code extensions run only where the sandbox works; elsewhere only declarative extensions run.
  `cyx dev` is the one exception: it runs the author's own local folder without the OS sandbox, still behind
  Wasm and the broker, with a DEV badge. Otherwise authors on that platform could not build anything.

## 4. The manifest

`codeyard-extension.toml` is the only required file. Most extensions need under 15 lines.

```toml
[extension]
id      = "dogan.deploy-status"         # <publisher>.<name>, lowercase a-z 0-9 -
version = "0.1.0"
name    = "Deploy Status"
license = "MIT"

[code]
entry = "src/main.ts"                   # or "Cargo.toml"; omit for declarative extensions

[permissions]
read = ["vercel.json"]
net  = ["api.vercel.com"]

[activation]
workspace-contains = ["vercel.json"]    # the only event you ever write by hand

[[commands]]
id    = "refresh"                       # becomes dogan.deploy-status.refresh
title = "Deploy: Refresh"

[[views]]
id       = "deployments"
title    = "Deployments"
location = "sidebar"                    # sidebar | bottom | editor
```

| Table / field | Required | Notes |
| --- | --- | --- |
| `extension.id`, `version`, `name`, `license` | yes | `description`, `repository`, `categories`, `icon` are optional |
| `code.entry` | for code | `Cargo.toml` means Rust, `.ts`/`.js` means TypeScript. Omitted for themes, snippets, keymaps, languages and schemas |
| `permissions` | if used | `net = ["host"]`, `read = ["glob"]`, `run = ["vercel"]`. `why = "..."` is required for High-risk sets; `cyx dev` warns on the first build, not at publish |
| `activation` | rarely | Inferred from commands, views, languages, URI handlers and auth. The only manual form is `workspace-contains`. There is no `*` and no `startup` |
| contribution ids | — | Always local and prefixed with the extension id. Another extension's command is referenced by full id and needs a `dependencies` entry; core commands use `core:` |
| `when`, `enablement` | — | Keys, `== != && \|\| !`, literals. Public keys: `editor`, `terminal`, `file-tree`, `scm`, `search`, `panel`, `workspace.trusted`, `editor.language`, `ext.<key>` |
| `dependencies` | — | `{ "other.ext" = "1.2" }`, a minimum version. Missing ones are offered for install |
| `untrusted` | — | Default: runs in untrusted folders with no outbound permissions. `false` means it does not run there |
| `l10n/<code>.tsv` | — | `English<TAB>Translation`, the core's format. English is the fallback |

Authors never write `engines`, `tier`, `kind`, `runs-on` or an API version. `cyx` computes the minimum
Codeyard version from the API items and contribution points used (each has a `since`), with no upper bound.

## 5. API and contributions

### 5.1 Contribution points

| Contribution | Needs code | Notes |
| --- | --- | --- |
| `themes` | no | Codeyard or VS Code colour theme JSON; contrast always repaired with `guard_contrast`. Id `ext:<publisher.name>/<file stem>` |
| `snippets` | no | VS Code snippet format |
| `keymap-preset` | no | Vim, Emacs, Sublime: a file in `keybindings.jsonc` format. Chosen with the `keymap.preset` setting; may override defaults |
| `keybindings` | no | For your own or core (`core:workbench.toggleZenMode`) commands; no permission, since only the user's key press runs them. A key a built-in binding or the chosen preset uses is never taken: the binding is left out with a warning. `when` takes `editor`, `terminal`, `file-tree`, `scm`, `search` and `editor.language == '…'` |
| `languages` | no | File names and extensions, a built-in grammar to highlight with, comment tokens. Only for files no built-in language claims (`files.associations` changes a built-in one) |
| `json-schemas`, `yaml-schemas`, `settings` | no | Settings appear under "Extensions › Name" |
| `language-servers` | no | Start a program from `PATH` where the files are (so also over SSH), so they need `run` and a trusted workspace. Entered before the user's `lsp.servers`; the user's entry for the same language wins |
| `commands`, `menus`, `status-bar`, `views` | yes | |
| `formatters`, `code-actions`, `diagnostics`, `decorations`, `tasks` | yes | |
| `hover`, `completion`, `code-lens`, `inlay-hints`, SCM and PR providers | yes | Phase 3 |

### 5.2 The API

One WIT package (`crates/ext-api/wit`). Authors use an SDK and never see WIT.

- **Rust:** `codeyard-extension`, target `wasm32-wasip2`, plain cargo.
- **TypeScript:** `@codeyard/extension`, componentized with ComponentizeJS (StarlingMonkey). All calls return
  Promises. Global `fetch` is provided and mapped to `net`, including streamed response bodies. Pure-JS npm
  packages that use `fetch` work (Octokit, zod); Node built-ins (`fs`, `child_process`, `net`) do not.
  Source maps are applied to traces in the Inspector.
- **No permission needed for:** log, notices, quick-pick, input box, progress (all show the publisher), the
  extension's own views, status items, storage and settings, `env` (version, language, theme kind, platform),
  commands of dependencies, and `open-external` for `https:` URLs from a user action (command or link click).
- **Documents.** A provider receives its document. A command the user started can read and edit the active
  document and selections until it returns (Chrome's `activeTab`). Anything else, including background reads
  of open documents, needs `read` on matching globs. `.env*` and `.git/` must be named explicitly.
- **Time, one rule for authors:** providers are cancelled when the user moves on, so answer fast; long work
  runs in the background and shows progress. `ctx.spawn(program, args)` streams output as events.
  `ctx.every(seconds)` polls (minimum 30 s, paused while the window is unfocused); an active timer keeps the
  exthost alive. Numbers live in the generated `reference/limits.md`.
- **Errors.** `denied { permission, why }` with `why` one of `not-declared`, `revoked-by-user`,
  `untrusted-workspace`, `revoked-extension`; the SDK formats it with the fix. Others: `not-found`,
  `cancelled`, `timeout`, `invalid`, `other`.
- **State.** `storage` is a persisted key-value store and the only persistence concept. In `cyx dev`, hot
  reload keeps in-memory state automatically.
- **Versioning.** `0.x` during beta, each break with a `cyx` error and migration note. From `1.0`, minors only
  add, items carry `@since`, and a major stays supported for 24 months after the next ships.

### 5.3 Declarative UI

A view is a tree of 22 elements: column, row, text, heading, icon, image, button, text-input, checkbox, select,
list, tree, table, badge, progress, markdown, code, key-value, banner, link, divider, empty-state. There is no
colour or font control: elements take tones (neutral, accent, success, warning, danger, muted) and spacing
steps 0–4, so themes, high contrast and screen readers work with no author effort. `image` takes bytes or a
URL on a `net` host.

- `ui::invalidate(view)` makes the host call `render` and diff by `key`. Input state lives in the host.
- Lists, tables and trees are virtualised; trees load one page at a time.
- Every extension surface shows the publisher id. Consent, trust and sign-in dialogs are drawn in a layer
  extension UI cannot reach.
- There are no webviews. Markdown previews, Mermaid and chart extensions are not possible in v1; the docs say
  so up front.

## 6. Permissions and security

### 6.1 Permissions

| Permission | Allows | Consent text |
| --- | --- | --- |
| `read` / `write` | workspace files matching globs, including open documents | "Read vercel.json" |
| `net` | HTTPS to listed hosts. A wildcard directly under a public suffix counts as any host | "Connect to api.vercel.com" |
| `auth` | requests signed by an account; the extension never sees the token | "Act as your GitHub account" |
| `secrets` | its own keychain entries | "Store secrets in your keychain" |
| `run` | listed programs, with any arguments | "Run vercel with any arguments" |
| `clipboard`, `terminal` | read or write the clipboard; type into a terminal | |
| `commands` | listed public core commands | Each core command declares the permission it implies; the caller must hold it too |

One function, `risk(permission_set)` in `ext-manifest`, decides everything risk-related: whether `why` is
required, the banner on the consent sheet, whether the registry needs human review, and what
`cyx publish --dry-run` prints. Something that reads private data (a wide `read`, a `read` of credential files
such as `.env*`, `.git/` or keys, `clipboard` read, `secrets`, `auth`) combined with something that sends (`net`,
`run`, `write`, `terminal`) is High. Anything that can run code as the user is High: `run`, `terminal`, and
`write` to anything wide or to files other tools execute (hooks, build files, editor tasks). A glob is wide when
it has `**`, a wildcard in a folder part, or a file part other than `name*.ext`. Text shown on the sheet may not
contain control, bidirectional or zero-width characters. Authors never see the levels, only the sheet that
`--dry-run` prints.

Interpreters and shells (`sh`, `node`, `python`, …) are allowed only when the script they run ships inside the
signed package; a language server wrapper is therefore a normal High-risk extension, not a special case.

### 6.2 What is enforced

- **Wasm.** A component can call only what it imports; imports must fit permissions. One function in `ext-pkg`
  checks this and is called by `cyx`, the registry and the installer. Revoked permissions link to stubs.
- **Broker (app).** Every file, network, process, account and command call is checked against
  grants ∩ workspace trust, then the path, host or program, before `Host` is called.
- **Processes.** The broker resolves the program to an absolute path on the target host, refuses paths under
  a workspace root, and passes an explicit environment (proto v12 adds `Which` and `EnvSpec`).
- **Network.** In-process hyper + rustls (already linked through GPUI). It checks the real peer address of
  every connection, so private, CGNAT, link-local, metadata, NAT64 and IPv4-mapped ranges are blocked. No IP
  literals, at most 5 re-checked redirects, environment proxies ignored. WebSockets are not offered in v1.
- **IPC.** `ext-proto` has its own frame cap (call limit + 64 KiB) and a per-process byte-rate budget.
- **Parsed data.** Themes, schemas and snippets are size-capped and fuzzed in CI. "No
  extension can crash the app" is a target backed by fuzzing and hostile fixtures, not a guarantee.

### 6.3 What the user sees

One consent model (permission set + diff + risk) renders the install sheet, the update card, `cyx dev
--as-user` and `cyx publish --dry-run`, so the text never diverges.

- **Install.** One sheet with permissions in plain words and the author's `why` as an attributed quote (200
  characters, no links). High risk adds a red banner. Allow is never the default button. Typing the extension
  id is required only for native programs.
- **Run time.** Undeclared access is denied, not prompted. The app asks only on first account use, first
  `clipboard` read per session, and first process on a remote host per session.
- **Updates.** Fewer permissions apply silently. More permissions keep the old version running and show a card
  with only the difference. A new publisher identity or source shows the full sheet.
- **Publisher identity** is one value (registry, numeric repository id, owner id, workflow ref). The registry's
  trusted-publisher record, the client's pin per extension id, and the grant key all use it. A sideloaded
  package with the same id starts with no grants.
- **Untrusted folders.** Declarative extensions apply. Code may read what it declared and show UI but cannot
  send, write or run; extension language servers do not start. The dev window shows the
  trust state, and errors name `untrusted-workspace` with the fix.
- **Management.** Per-permission switches and a recent-denials list on the extension page (in memory only).
- **Crashes.** Three crashes within a minute disable the extension and show **Reinstall previous version** and
  **Copy report** (version, trap location, mapped stack). If `repository` is set, **Report** opens a prefilled
  issue there. Nothing is sent automatically.

### 6.4 Auth

`auth` exists so integrations never ask users to paste tokens. Phase 2 ships a built-in GitHub provider and a
generic OAuth 2 PKCE flow: the manifest declares the authorize and token URLs, the app runs the browser flow
through `codeyard://ext/<id>/callback`, keeps the token in the keychain, and signs requests to the declared
hosts. The extension never sees the token. Pasting a raw token into `secrets` stays possible and is High risk.

## 7. Remote

> Wasm runs on the UI machine. Files, processes and git go through the window's `Host`. `net` requests leave
> from the UI machine. Native programs run on the workspace host.

- There is no `extensionKind` or `runs-on`. The agent never hosts Wasm: credentials stay local, server CPU
  stays the user's, and one rule is easier to learn.
- Each remote host call is one SSH round trip. Open documents come from the app's rope at no cost;
  `read-many`, `find-files` and `search` batch work. Provider deadlines add the measured RTT for up to 4 round
  trips. The Inspector splits each call into guest, I/O and overhead time.
- Native programs come only from per-platform packages (`<id>-<version>-<triple>.cyx`, static on Linux). The
  app pushes verified bytes with the same temp-file, `sha256sum`, `mv` path the agent install uses. The server
  downloads nothing.
- On disconnect, instances stay alive and host calls return `HostUnavailable`. Containers and WSL work the
  same way. Agent size limits live in `docs/remote.md`; extensions add nothing to the agent.

## 8. Registry and publishing

**Yard** is static files on a CDN plus a small publish API (axum + Postgres) holding publishers and
trusted-publisher records.

`cyx publish`, or a tag push with the scaffolded workflow:
1. `cyx` checks, builds and packages a deterministic `.cyx` (ustar + gzip, sorted, mtime 0). The content
   digest is the sha256 of the uncompressed tar.
2. It signs keyless: in GitHub Actions the workflow's OIDC identity gets a short-lived Sigstore certificate;
   on a laptop a browser login to GitHub does the same. No author holds a key.
3. The API matches the OIDC claims against the trusted-publisher record and accepts the upload.
4. The registry runs the same checks as `cyx` (schema, package rules, imports fit permissions, permission
   digest consistent). Ids are ASCII; confusable and near-duplicate names are rejected automatically. Human
   review applies only to the first native program from a publisher.
5. The version is added to the signed index and the publisher gets a notice.

Keys. An offline root (owner holds 2 of 3 keys, the third for recovery) signs the online index key with a
90-day expiry. The index is re-signed daily and expires after 72 h. After an online-key compromise the root
signs a new key, and clients accept it on their next fetch. The root also pins the Fulcio CA and Rekor key, so
clients verify publisher signatures offline. SECURITY.md states who holds what.

Before consent the client checks: index signature and expiry, package hash and content digest, publisher
signature, publisher identity against its pin, version status, package rules and imports. Any failure installs
nothing.

Updates and privacy:
- The client downloads the whole compressed signed index once a day with a conditional GET (ETag). It also
  serves Market search. Every client fetches the same bytes, so the installed set never leaves the machine.
- Each version has one `status` in the index: `ok`, `deprecated(replacement)`, `yanked` or `revoked`. Revoked
  versions stop at once with their dependents (target: online clients within 24 h). Yanked versions are hidden
  from installs, and users on them roll back to the previous version immediately.
- Updates that run code wait 48 h; data-only packages update at once. The user can press **Update now** on one
  extension. `cyx publish` says when users will get the version. The registry security role can shorten the
  wait for an advisory.
- `cyx publish --pre-release` publishes to a channel users opt into per extension, with no wait.
- If the index is older than 72 h, installs and updates are blocked until the user confirms; installed
  extensions keep running.
- `extensions.lock.jsonc` (config dir) is the only record of what is installed: requested version, resolved
  version, content digest, per-triple digests. It can be synced with dotfiles.
- Version numbers are never reused. Stats are download counts kept as `(id, version, day)` with no logs,
  labelled "including updates". No ratings or reviews. READMEs render natively without remote images.
- Limits are registry policy: `.wasm` 64 MiB, package 64 MiB, per-triple package 128 MiB.

## 9. Developer experience

Goals: a theme or snippet extension in under 3 minutes with only Codeyard installed; a command in under 5
minutes with Rust or Node installed; every error says what is wrong, where, and what to change.

1. **Start.** Palette → **Extensions: New Extension…**, or `cyx new command my-tools`. Both use the same
   templates (the palette calls `cyx`). A dev window opens.
2. **Edit.** `cyx dev` rebuilds on save and hot-reloads. Declared permissions are granted for the session
   (local folder, DEV badge, `dev:` source), with one confirmation per session only if the set is High.
3. **Debug.** **Developer: Extension Inspector** ships in release builds and is local only: why and when the
   extension activated, every call split into guest, I/O and overhead, each denial with **Copy fix**, the live
   UI tree with highlighting, logs and traps with file:line. Attaching a debugger pauses deadlines.
4. **Test.** Unit tests run natively against a mock host (fake files, recorded processes, fixture HTTP,
   virtual clock): `cargo test` for Rust, `npm test` with `@codeyard/extension/testing` for TypeScript.
   `cyx test` runs the real `.wasm`.
5. **Publish.** `cyx new` writes `.github/workflows/release.yml` with actions pinned by SHA. The first
   `cyx publish` registers the trusted publisher; after that, pushing a tag is the whole release.

Author commands are `cyx new`, `cyx dev`, `cyx test`, `cyx publish [--dry-run|--pre-release]`. Advanced:
`cyx check [--fix]`, `cyx package`, `cyx doctor`, `cyx explain CY0201`, `cyx import-vscode <dir|.vsix>`
(themes, snippets, keymaps, language configuration; lists what it could not convert), and user
commands `install|list|uninstall|sync`.

`cyx` is a small GPUI-free console binary shipped in the app bundle (a Windows GUI exe cannot write to a
terminal). **Extensions: Install cyx Command in PATH** adds it; CI uses `codeyard/setup-cyx`.

Errors have one format everywhere:

```
error[CY0201]: net access to "api.vercel.com" is not declared
  --> codeyard-extension.toml
   = fix: add to [permissions]:  net = ["api.vercel.com"]
   = docs: https://codeyard.dev/docs/extensions/errors#CY0201
```

Docs in `docs/extensions/`: README, quickstart, one guide each for themes, snippets and keymaps, languages,
commands, views, settings, permissions, testing, debugging, publishing (including the 48 h wait and
pre-releases), and `from-vscode.md`; generated references for manifest, contributions, API, CLI, errors and
limits. Every snippet is a real extension in `examples/extensions/` that CI builds and loads, and every error
code has a fix line, a docs anchor and a test.

## 10. Performance targets

There is no app binary size budget. Sizes are recorded each release for information.

Each release builds the previous tag and the candidate on the reference Mac and runs them interleaved, 20 runs
each. A metric blocks the release when it regresses past its budget and beyond the noise of the baseline (a
bootstrap interval of the same statistic), or crosses its ceiling; `--aa` checks the machine's noise. Statistics
and the metric catalogue (`perf/metrics.json`) are described in `scripts/perf/README.md`. PRs gate only on deterministic metrics; nightly runs open issues; Windows rows are
`warn` until there is a physical Windows reference machine. Ceilings are provisional until the first baseline,
then set to `min(stated, baseline × 1.25)` and only lowered. A waiver is one line in the release notes.
`CODEYARD_PERF=<file>` writes spans locally; when off, a span costs one relaxed atomic load.

| Metric | Stat | Budget / release | Ceiling |
| --- | --- | --- | --- |
| Startup cost of 100 installed, inactive extensions | p95 delta | +1 ms | 5 ms, 0 processes, 0 requests |
| Keystroke to frame, 5 query packs + 2 decoration providers active | p95 / p99 | +10% | same as without extensions (8 / 16 ms) |
| Idle memory, 100 installed, none active | median | +2 MB | welcome scene + 3 MB |
| Activation, warm (process running, code cached) | p95 | +10% | 10 ms |
| Activation, cold (spawn + sandbox + load) | p95 | +10% | macOS 150 ms, Windows 400 ms |
| Provider platform overhead (excl. guest and I/O) | p95 | +10% | local 3 ms; remote 5 ms + RTT |
| Exthost memory per extension (idle, Rust hello) | median | +10% | 25 MB; TypeScript set by the Phase 2 spike |
| Crash to providers answering again | p95 | +15% | 2 s |
| Hot reload (2 MB Rust, compiler excluded) / declarative | p95 | +15% | 500 ms / 200 ms |
| Market search, keystroke to results, 10k entries | p95 | +10% | 16 ms, off the UI thread |

Pass/fail gates: every hostile fixture is denied or trapped; fuzz targets run 10 minutes per PR and 4 hours
nightly with no new crash; the scripted "new theme → visible" check passes in 10 s.

## 11. Codebase integration

| Crate (all GPUI-free) | Contents |
| --- | --- |
| `ext-manifest` | manifest types, validation, `risk()`, `limits`, `when` parser, error catalogue |
| `ext-pkg` | `.cyx` reader/writer, content digest, path safety, import check |
| `ext-core` | install transaction, store, lock, contribution index, grants ∩ trust, index and signature verification |
| `ext-proto`, `ext-ui`, `ext-api` | app ↔ exthost frames; UI tree model and diff; WIT and generated docs |
| `ext-host` | the `codeyard-exthost` binary: engine, sandbox, `--compile`. The only crate that links wasmtime |
| `cyx`, `ext-sdk`, `ext-test` | CLI, Rust SDK, mock host |

App code lives in `ext_*.rs` topic files (broker, Extensions view, consent sheet, UI renderer, Inspector) and
`perf.rs`. The workbench gets one `ext` field. `theme`, `snippets.rs`, `syntax`, `schema`, `lsp.servers` and
`dap.rs` gain an extension source. Extension `when` expressions map public contexts to GPUI contexts inside the
app, so GPUI names stay private. Table-level details belong in the PRs.

Disk: `<data>/extensions/store/<digest>/` (read-only, content-addressed), `compiled/<engine-hash>/`, and
`data/<id>/` (50 MiB). GC keeps the current and previous digest of each extension and deletes compiled code for
engine hashes other than the current one.

Settings: `extensions.autoUpdate` (`off | notify | install`, default `install`), `extensions.allowSideload`,
`extensions.registries` (user-only).

## 12. Key decisions

| # | Decision | Choice | Why |
| --- | --- | --- | --- |
| 1 | How code runs | Wasm components in `codeyard-exthost`, never in the app | Sandbox by construction; no JIT or third-party code in the app |
| 2 | Process model | One process per active code extension, exits when idle | One rule; an escape gets one extension; crash blame is trivial |
| 3 | OS sandbox | Required for installed code; `cyx dev` may run the author's own folder without it | Kept for safety: Wasm alone is one bug from the user's files. Dev exception so every platform can author |
| 4 | Manifest | One TOML file, activation inferred, no engines field, always-prefixed ids | VS Code's shape with less to repeat and no namespace traps |
| 5 | Permissions | Declared, enforced in the app broker, one `risk(set)` function | One place to audit; read + send is what makes exfiltration |
| 6 | UI | 22 declarative elements drawn by GPUI, no webviews | No XSS; themes and accessibility for free; low memory |
| 7 | Remote | Wasm on the UI machine; host calls through `Host` | Local and SSH share one path; the agent stays untouched |
| 8 | Signing | Keyless Sigstore from CI or browser, verified offline, plus a registry-signed index | Authors manage no keys; a compromised registry cannot forge a publisher |
| 9 | Updates | Whole index by ETag; 48 h wait for code; yank rolls back; pre-release channel | Kept for safety: the wait is the main defence against a hijacked repo; yank and pre-release keep hotfixes fast |
| 10 | Gates | Relative budgets + user-felt ceilings; no app size budget | Users feel time and memory, not bytes |

## 13. Later

- Icon themes, `.scm` queries for extension languages, debug adapters as declarative contributions: each needs its own engine (data-driven icons, runtime queries, launch configurations); they come after Phase 0.
- Index deltas: when the compressed index passes 5 MB.
- Version ranges and a full resolver (PubGrub): when dependency chains appear in practice.
- Per-extension generated build world (undeclared API as a compile error): the import check covers it now.
- Long-lived publisher keys, private registries, mirrors, managed policy: after the public registry is stable.
- Malware scanning for native programs: with open native publishing, after Phase 3.
- Sharing one process among network-less extensions: only if per-process memory misses its ceiling.
- WebSockets, and a sandboxed canvas for previews and charts: after the element set is proven.
- Go and Python SDKs, source-level Wasm breakpoints: when each meets its ceilings or is proven on macOS arm64.

## 14. Roadmap

| Phase | Scope | Exit |
| --- | --- | --- |
| **0: Measure, then declarative** (5 wk) | Week 1: spans, `scripts/perf/*`, baseline of today's editor. Then `ext-manifest`, `ext-pkg`, `ext-core`, lock, declarative contributions, Extensions view, `--no-extensions`, `cyx new/dev/check/package`, `import-vscode` for themes and snippets, quickstart. Extension language servers count as `run` from the start | A/A run shows no false regression; startup and idle-memory rows green; theme check ≤ 10 s; fuzz green |
| **1: Signed registry** (5 wk) | Yard, keyless publish, client verification, version status, `codeyard/publish` action, Market with local search. Data-only publishing open to everyone | Search row green; 0 registry requests for users with no extensions; canary revocation within 24 h |
| **2: Code extensions, API 0.x** (12 wk) | Entry: notarized release pipeline. Week 1 spike: spawn, memory per process, compile, hot reload, and a go/no-go for TypeScript against the memory and cold-activation ceilings (if no-go, TypeScript follows Rust by one release). Then exthost, broker, macOS and Linux sandboxes, SDKs, commands, views, formatters, code actions, GitHub and OAuth auth, Inspector, hot reload, `cyx test`. Windows code ships when AppContainer passes on physical hardware | Activation, provider, memory, crash and hot-reload rows green; hostile fixtures pass; 0 extension-caused app crashes; informal sessions with new authors before GA |
| **3: GA, API 1.0** | Hover, completion, code lens, inlay hints, SCM and PR providers; native programs by invitation with human review | All `block` rows green for 2 releases with no waivers |

## 15. Decisions taken on the open questions

Decided by the owner on 2026-10-28.

1. **Windows reference machine.** None for now. Windows performance rows stay `warn`, and Windows code
   extensions ship only after AppContainer passes on physical hardware. Declarative extensions ship on Windows
   from Phase 0.
2. **Memory per active extension.** 25 MB is accepted as the ceiling. If the Phase 2 spike measures more,
   network-less extensions share one process (the item in [Later](#13-later) moves into Phase 2); the ceiling is
   not raised.
3. **Registry policy.** The 48 h wait for code updates, open data-only publishing in Phase 1 and no ratings are
   accepted.
4. **Root keys.** The owner holds 2 of 3 root key shares; the third is an offline backup. This is disclosed in
   SECURITY.md before Phase 1, and a second holder is added when one is available.
