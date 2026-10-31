# API reference

<!-- Generated from crates/ext-api/wit: run `UPDATE_DOCS=1 cargo test -p ext-api reference` after changing it. -->

`codeyard:api@1.0.0`. Authors use an SDK, which wraps these calls; this page says what each one does and which permission it needs. Calls that return a `ticket` finish later: the SDK turns them into async functions (Rust) or Promises (TypeScript).

Every item has the API version that added it. Within a major version a minor only adds: an extension that uses only items of 1.0 runs on every 1.x. An item newer than its interface says so with *since*; `cyx` works out the oldest Codeyard an extension needs from the items it uses.

## `log`

Messages for the extension's log, shown in the Inspector.

Needs no permission. Since 1.0.0.

- type `level`: enum `trace | debug | info | warn | error`
- `log(level: level, message: string)`

## `env`

What the extension runs in. Known at activation, so these calls are cheap.

Needs no permission. Since 1.0.0.

- type `theme-kind`: enum `light | dark | high-contrast-light | high-contrast-dark`
- type `platform`: enum `macos | linux | windows`
- type `environment`: record `{ codeyard-version: string, api-version: string, language: string, theme: theme-kind, platform: platform, remote: bool, workspace-trusted: bool }`
- `info() -> environment`

## `settings`

The extension's own settings, as declared in its manifest.

Needs no permission. Since 1.0.0.

- `get(key: string) -> option<string>`: The value as JSON, or none when unset and without a default. Changes arrive as `request.settings-changed`.

## `timers`

Repeating timers. An active timer keeps the exthost running.

Needs no permission. Since 1.0.0.

- `every(seconds: u32) -> u64`: Delivers `request.timer(id)` every `seconds` (at least 30), paused while the window is unfocused.
- `clear(id: u64)`

## `ui`

The extension's views.

Needs no permission. Since 1.0.0.

- `invalidate(view: string)`: Ask for the view to be drawn again: the host sends `request.render` and diffs the result by key.

## `pull-requests`

Pull (merge) request providers: `[[pull-requests]]` in the manifest. The host asks with `request.pull-requests` and the extension answers with `response.pull-requests`. The provider calls its forge with `auth.fetch`, so it never sees the account's token.

Needs no permission. Since 1.0.0.

- type `remote`: record `{ provider: string, name: string, url: string, host: string, path: string, account: option<string> }`. A repository remote whose host the provider claims.
- type `state`: enum `open | draft | merged | closed`
- type `summary`: record `{ id: string, reference: string, title: string, author: string, state: state, source-branch: string, target-branch: string, updated: u64, url: option<string> }`
- type `check-state`: enum `pending | success | failure | neutral`
- type `check`: record `{ name: string, state: check-state, url: option<string> }`
- type `details`: record `{ summary: summary, description: string, checks: list<check>, labels: list<string> }`
- type `file-status`: enum `added | modified | deleted | renamed`
- type `changed-file`: record `{ path: string, old-path: option<string>, status: file-status }`
- type `file-contents`: record `{ base: option<string>, head: option<string> }`. One changed file's text on both sides, for a diff.
- type `comment`: record `{ author: string, body: string, created: u64, path: option<string>, line: option<u32> }`
- type `checkout`: record `{ fetch-url: option<string>, ref: string, branch: string }`. How to check a request out. The host runs `git fetch <from> <ref>` and creates or resets `branch` on what it fetched, on the machine the repository is on.
- type `query`: variant `list(remote) | details(tuple<remote, string>) | files(tuple<remote, string>) | contents(tuple<remote, string, changed-file>) | comments(tuple<remote, string>) | add-comment(tuple<remote, string, string>) | checkout(tuple<remote, string>)`
- type `answer`: variant `list(list<summary>) | details(details) | files(list<changed-file>) | contents(file-contents) | comments(list<comment>) | checkout(checkout) | done`

## `types`

Types shared by every interface. Importing it needs no permission.

Needs no permission. Since 1.0.0.

- type `why`: enum `not-declared | revoked-by-user | untrusted-workspace | revoked-extension`. Why a call was refused.
- type `denied`: record `{ permission: string, why: why }`
- type `error`: variant `denied(denied) | not-found(string) | cancelled | timeout | invalid(string) | other(string)`
- type `ticket`: `u64`. A host call that finishes later. Its result arrives at `guest.complete`, streamed data before that at `guest.event`.
- type `position`: record `{ line: u32, character: u32 }`. Zero-based line and UTF-16 column, as in JavaScript strings and the Language Server Protocol.
- type `range`: record `{ start: position, end: position }`
- type `text-edit`: record `{ range: range, new-text: string }`
- type `uri`: `string`. A file or folder. Opaque to extensions: the same code works on local and SSH folders.
- type `tone`: enum `neutral | accent | success | warning | danger | muted`
- type `http-request`: record `{ method: string, url: string, headers: list<tuple<string, string>>, body: list<u8>, timeout-ms: option<u32> }`. An HTTPS request (`net.fetch`, `auth.fetch`). The body is sent in one piece, up to the call limit.
- type `http-head`: record `{ status: u16, headers: list<tuple<string, string>> }`. Arrives as `stream-event.http-head` before the body's `data` events.
- type `diagnostic-severity`: enum `error | warning | information | hint`
- type `diagnostic`: record `{ range: range, severity: diagnostic-severity, message: string, code: option<string> }`
- type `document`: record `{ uri: uri, language: string, version: u32, text: string }`
- type `file-stat`: record `{ is-dir: bool, size: u64, modified: u64 }`
- type `search-match`: record `{ uri: uri, range: range, line: string }`
- type `file-result`: record `{ uri: uri, contents: result<list<u8>, error> }`. A file read by `workspace.read-many`: its bytes, or why it could not be read.
- type `account`: record `{ provider: string, label: string }`
- type `completion`: variant `done | failed(error) | bytes(list<u8>) | text(option<string>) | json(string) | chosen(list<u32>) | files(list<file-result>) | uris(list<uri>) | stat(file-stat) | document(option<document>) | ranges(list<range>) | exit(option<s32>) | account(account)`. The result of a ticket.
- type `stream-event`: variant `http-head(http-head) | data(list<u8>) | stdout(list<u8>) | stderr(list<u8>) | search-match(search-match)`. Streamed data for a ticket, before its completion.

## `tickets`

Calls the host has not answered yet.

Needs no permission. Since 1.0.0.

- `cancel(ticket: ticket)`: Stop waiting: the ticket completes with `cancelled`, and a running process is killed.

## `window`

Notices, picks, input, progress, status items and links. Every one shows the publisher.

Needs no permission. Since 1.0.0.

- type `severity`: enum `info | warning | error`
- type `pick-item`: record `{ label: string, detail: option<string>, picked: bool }`
- type `pick-options`: record `{ title: option<string>, placeholder: option<string>, many: bool }`
- type `input-options`: record `{ title: option<string>, prompt: option<string>, value: option<string>, placeholder: option<string>, password: bool }`
- type `status-item`: record `{ id: string, text: string, tooltip: option<string>, command: option<string>, tone: tone }`
- `show-notice(severity: severity, message: string, actions: list<string>) -> ticket`: Completes with `chosen`: the index of the action clicked, or empty when dismissed.
- `quick-pick(items: list<pick-item>, options: pick-options) -> ticket`: Completes with `chosen`: the picked indices, or empty when dismissed.
- `input-box(options: input-options) -> ticket`: Completes with `text`: what the user typed, or none when dismissed.
- `start-progress(title: string, cancellable: bool) -> u64`: Shows progress until `end-progress`. Cancelling it from the UI arrives as `request.cancel-progress`.
- `report-progress(id: u64, message: option<string>, percent: option<u8>)`
- `end-progress(id: u64)`
- `set-status-item(item: status-item) -> result<_, error>`
- `remove-status-item(id: string)`
- `open-external(url: string) -> ticket`: Opens an `https:` URL in the browser. Only while handling a user action (a command or a link click): the host checks that, so the ticket completes with `done` or `failed(denied)`.

## `commands`

Running commands.

Needs no permission. Since 1.0.0.

- `execute(command: string, args-json: string) -> ticket`: The extension's own command, a dependency's, or a core command (`core:…`) the manifest lists. Arguments and result are JSON; completes with `json`.

## `storage`

A persisted key-value store in the extension's data folder (50 MiB in all). Answered by the exthost itself.

Needs no permission. Since 1.0.0.

- `get(key: string) -> option<list<u8>>`
- `set(key: string, value: list<u8>) -> result<_, error>`: Fails with `invalid` when the store would grow past its limit.
- `delete(key: string)`
- `keys() -> list<string>`

## `documents`

Open documents. A command the user started may read and edit the active document and its selections until it returns; anything else needs `read` (and `write` for edits) on the document's path.

Needs no permission. Since 1.0.0.

- `active() -> ticket`: Completes with `document`: the active editor's document, or none.
- `selections() -> ticket`: Completes with `ranges`: the active editor's selections.
- `read(uri: uri) -> ticket`: Completes with `document`: an open document, or none when it is not open.
- `apply-edits(uri: uri, version: u32, edits: list<text-edit>) -> ticket`: Applies edits made against `version`. Completes with `done`, or `failed(invalid)` when the document changed.
- `set-selections(uri: uri, selections: list<range>) -> ticket`

## `languages`

Diagnostics and decorations the extension shows on documents.

Needs no permission. Since 1.0.0.

- type `decoration-place`: enum `gutter | line | after`
- type `decoration`: record `{ range: range, place: decoration-place, tone: tone, text: option<string>, hover: option<string> }`
- `set-diagnostics(uri: uri, diagnostics: list<diagnostic>)`: Replaces this extension's diagnostics for `uri`.
- `clear-diagnostics(uri: option<uri>)`: Removes this extension's diagnostics for `uri`, or everywhere.
- `set-decorations(uri: uri, set: string, decorations: list<decoration>)`: Replaces one named set of decorations on `uri`.

## `workspace`

Reading workspace files. Needs `read` on the paths.

Needs `read = ["<glob>"]` in `[permissions]`. Since 1.0.0.

- type `search-query`: record `{ pattern: string, regex: bool, case-sensitive: bool, whole-word: bool, include: list<string>, max: u32 }`
- `folders() -> list<uri>`: The workspace's folders.
- `read-file(uri: uri) -> ticket`: Completes with `bytes`.
- `read-many(uris: list<uri>) -> ticket`: Completes with `files`, one per uri, in one round trip.
- `stat(uri: uri) -> ticket`: Completes with `stat`.
- `find-files(glob: string, max: u32) -> ticket`: Completes with `uris`: files matching `glob` under the workspace folders, at most `max`.
- `search(query: search-query) -> ticket`: Streams `search-match` events, then completes with `done`.

## `workspace-write`

Changing workspace files. Needs `write` on the paths.

Needs `write = ["<glob>"]` in `[permissions]`. Since 1.0.0.

- `write-file(uri: uri, contents: list<u8>) -> ticket`
- `delete(uri: uri) -> ticket`
- `rename(from: uri, to: uri) -> ticket`
- `create-directory(uri: uri) -> ticket`

## `net`

HTTPS to the hosts listed in `net`. Requests leave from the machine Codeyard runs on.

Needs `net = ["<host>"]` in `[permissions]`. Since 1.0.0.

- `fetch(request: http-request) -> ticket`: Streams `http-head`, then the body as `data`, then completes with `done`.

## `auth`

Requests signed with an account listed in `auth`. The extension never sees the token.

Needs `auth = ["github"]` in `[permissions]`. Since 1.0.0.

- `session(provider: string, scopes: list<string>) -> ticket`: Signs in if needed (the first use asks the user). Completes with `account`. `provider` is an id from `auth`, or an instance account a `pull-requests.remote` carries.
- `fetch(provider: string, request: http-request) -> ticket`: Like `net.fetch`, signed for `provider` (as for `session`), to that account's hosts only.

## `secrets`

The extension's own entries in the system keychain. Needs `secrets`.

Needs `secrets = true` in `[permissions]`. Since 1.0.0.

- `get(key: string) -> ticket`: Completes with `text`.
- `set(key: string, value: string) -> ticket`
- `delete(key: string) -> ticket`

## `process`

Programs listed in `run`, on the workspace's host.

Needs `run = ["<program>"]` in `[permissions]`. Since 1.0.0.

- type `command`: record `{ program: string, args: list<string>, cwd: option<uri>, env: list<tuple<string, string>>, stdin: option<list<u8>> }`
- `spawn(command: command) -> ticket`: Streams `stdout` and `stderr`, then completes with `exit`. `tickets.cancel` kills it.

## `clipboard-read`

Reading the clipboard. Needs `clipboard = "read"` (or `"read-write"`); the first read per session asks.

Needs `clipboard = "read"` in `[permissions]`. Since 1.0.0.

- `read-text() -> ticket`: Completes with `text`.

## `clipboard-write`

Writing the clipboard. Needs `clipboard = "write"` (or `"read-write"`).

Needs `clipboard = "write"` in `[permissions]`. Since 1.0.0.

- `write-text(text: string) -> ticket`

## `terminal`

Typing into a terminal. Needs `terminal`.

Needs `terminal = true` in `[permissions]`. Since 1.0.0.

- `send-text(text: string) -> ticket`: Types `text` into the active terminal. Completes with `failed(not-found)` when there is none.

## `scm`

Source control providers (`[[scm]]`): what the Source Control view and the gutter show for a folder the provider claims. The host asks with `request.scm-status`, `request.scm-original` and `request.scm-validate`; buttons run the extension's own commands.

Needs no permission. Since 1.0.0.

- type `file-state`: enum `modified | added | deleted | renamed | untracked | ignored | conflicted`. How a file differs from what the provider last recorded.
- type `scm-action`: record `{ command: string, title: string, icon: option<string> }`. A button: one of the extension's own commands. It runs with the JSON arguments `{"provider": "<local id>", "root": "<folder uri>", "uris": ["<file uri>", …], "message": "<input box text>"}`.
- type `scm-resource`: record `{ uri: uri, state: file-state, detail: option<string> }`
- type `scm-group`: record `{ id: string, label: string, resources: list<scm-resource>, resource-actions: list<scm-action>, group-actions: list<scm-action>, hide-when-empty: bool }`
- type `scm-input`: record `{ placeholder: string, value: option<string>, accept: option<scm-action> }`
- type `scm-state`: record `{ head: option<string>, input: scm-input, groups: list<scm-group> }`
- `refresh(provider: string)`: Ask the host for `request.scm-status` again now, after the extension changed something itself.

## `responses`

What `guest.handle` answers with.

Needs no permission. Since 1.0.0.

- type `code-action`: record `{ id: string, title: string, edits: list<tuple<uri, list<text-edit>>>, command: option<string>, preferred: bool, fixes: list<diagnostic> }`
- type `task`: record `{ id: string, label: string, detail: option<string>, group: string }`
- type `lens-command`: record `{ title: string, command: string, args-json: string }`. What a code lens shows and the command a click runs.
- type `code-lens`: record `{ id: string, range: range, command: option<lens-command> }`
- type `inlay-hint`: record `{ position: position, label: string, tooltip: option<string> }`
- type `hover`: record `{ contents: string, range: option<range> }`. Shown in the hover popup next to the language server's, under the extension's name.
- type `completion-kind`: enum `text | method | function | constructor | field | variable | class | interface | module | property | unit | value | enum | keyword | snippet | color | file | reference | folder | enum-member | constant | struct | event | operator | type-parameter`. The icon a completion item shows, as in the Language Server Protocol.
- type `completion-item`: record `{ label: string, kind: completion-kind, detail: option<string>, documentation: option<string>, insert-text: option<string>, snippet: bool, range: option<range>, additional-edits: list<text-edit>, filter-text: option<string>, sort-text: option<string> }`
- type `completion-list`: record `{ items: list<completion-item>, incomplete: bool }`
- type `response`: variant `none | json(string) | view(string) | edits(list<text-edit>) | code-actions(list<code-action>) | tasks(list<task>) | ranges(list<range>) | hover(option<hover>) | completions(completion-list) | code-lenses(list<code-lens>) | inlay-hints(list<inlay-hint>) | scm(scm-state) | text(option<string>) | pull-requests(answer)`

## `requests`

Answers to `guest.handle`.

Needs no permission. Since 1.0.0.

- `respond(id: u64, result: result<response, error>)`: Answer request `id`. A request is answered once; late answers (after `guest.cancel`) are dropped.

## `host-requests`

What the host asks the extension.

Needs no permission. Since 1.0.0.

- type `format-options`: record `{ tab-size: u32, insert-spaces: bool }`
- type `request`: variant `command(tuple<string, string>) | render(string) | ui-event(tuple<string, string>) | format(tuple<document, option<range>, format-options>) | code-actions(tuple<document, range, list<diagnostic>>) | run-code-action(string) | provide-tasks | run-task(string) | settings-changed(list<string>) | timer(u64) | cancel-progress(u64) | open-uri(string) | hover(tuple<document, position>) | completion(tuple<document, position, option<string>>) | code-lens(tuple<document, range>) | resolve-code-lens(tuple<document, code-lens>) | inlay-hints(tuple<document, range>) | scm-status(tuple<string, uri>) | scm-original(tuple<string, uri>) | scm-validate(tuple<string, uri, string>) | pull-requests(query)`

## `guest`

What the extension exports. The SDKs implement it; authors never see it.

Exported by the SDK; authors do not call it. Since 1.0.0.

- type `activation`: record `{ reason: string }`
- `activate(activation: activation) -> result<_, string>`: Called once, before anything else.
- `deactivate()`: Called before the exthost stops. Keep it short: the process exits after a deadline.
- `handle(id: u64, request: request)`: A request; answer it now or later with `requests.respond(id, …)`. Must return quickly.
- `cancel(id: u64)`: The host no longer waits for request `id` (the user moved on).
- `complete(ticket: ticket, result: completion)`: A ticket finished.
- `event(ticket: ticket, event: stream-event)`: Streamed data for a ticket that has not finished.
