# Editor services

How the larger editor features work and where each one deliberately stops. User-facing descriptions are in the
README; the tests named here are the executable specification.

## JSON and YAML schemas (`crates/schema`, `workbench_schema.rs`, `editor_schema.rs`)

- **Parsing.** `doc` parses JSON (with comments and trailing commas) and YAML (`yaml-rust2`, multi-document) into a
  `serde_json::Value` plus a span for every node, so a violation at a JSON pointer maps back to a byte range. YAML
  custom tags (`!Ref`) mark nodes opaque and they are not validated. Aliases are expanded under a budget
  (`ALIAS_BUDGET`) and nesting is capped (`MAX_YAML_DEPTH`), so alias bombs and pathological nesting cannot exhaust
  memory.
- **Choosing a schema** (`ide::associate`): `$schema` in the file, a YAML modeline
  (`# yaml-language-server: $schema=…`), the `json.schemas` / `yaml.schemas` settings (patterns, relative paths
  resolved against the workspace root), then the SchemaStore catalog by file name.
- **Loading** (`store`): remote schemas are fetched with `curl` and cached on disk for 7 days (`schema.download`
  can turn downloads off); local schema files are read through a `FileSource`. In an SSH workspace the
  `FileSource` reads from the workspace host, so a relative `$schema` path means the server's file, not the
  laptop's.
- **Validation** (`validate`): `jsonschema` with a custom retriever backed by the store, format assertions off,
  at most 200 violations, regex engine limits ignored (a pathological schema pattern is not the document's fault),
  the root `$schema` key never reported as an unknown property.
- **Hover and completion** (`walk`, `cursor`): the walker follows `$ref`, `allOf`, `oneOf`/`anyOf` to describe the node
  under the cursor; the cursor context parser is lenient so completion works in documents that do not parse yet.
  Property lists are ordered required-first, then alphabetically.
- **Safety.** Everything public is wrapped in `guarded()`, which converts a panic inside a dependency into an
  ordinary "could not check this file" notice. `fuzz_tests` throws hostile input at every entry point.
  Large files get a hint instead of a check; meta-schemas stay silent.

## Spell check (`crates/spell`, `workbench_spell.rs`, `editor_spell.rs`)

- Hunspell dictionaries through `spellbook`. English (SCOWL) and a developer word list ship inside the binary; other
  languages are installed on request from the `wooorm/dictionaries` collection, cached, and added to
  `spelling.languages`. A word is correct if any listed dictionary accepts it.
- The tokenizer decides what is checked: prose in Markdown and plain text, comments and strings in code; it skips
  URLs, identifiers with digits or camel case, and treats combining marks as word characters.
- Suggestions are limited to words of at most 64 characters (`MAX_SUGGEST_WORD`) because Hunspell suggestion search
  on long junk takes seconds.
- Dictionary installs write a `.part` file and rename it; leftovers of an interrupted download are cleaned up.
  A missing or unreadable dictionary produces one notice, not one per file.
- The personal dictionary is a text file; "Add to dictionary" can also be session-only.

## Colour literals (`color.rs`, `editor_color.rs`)

Hex, `rgb()`, `hsl()` and CSS names are found case-insensitively, skipping URL fragments and identifiers. A swatch
is drawn before the literal; the picker rewrites the literal in HEX, RGB or HSL. A fuzz test round-trips every
format.

## Merge editor (`conflict.rs`, `merge.rs`)

`conflict::find` locates conflict blocks (including `diff3` ancestors). `merge::cards` turns them into cards with
Current, Incoming and optional Base panes and two lines of context that never swallow a neighbour's markers.
Resolution is an ordinary edit of the editor's buffer, so it is undoable and the page needs no model of its own.
The page holds a weak reference to its editor and closes when that editor is released. Rendering is capped at 40
cards and 200 lines per pane; the header always shows the true count and hidden cards appear as earlier ones are
resolved.

## GitHub pull requests (`github.rs`, `workbench_pulls.rs`)

Listing prefers `gh pr list` (private repositories, checks, review state) and falls back to the REST API with
`GITHUB_TOKEN`/`GH_TOKEN`. External commands run through `run_with_timeout`: no stdin, `GH_PROMPT_DISABLED=1`,
`GIT_TERMINAL_PROMPT=0`, killed after 30 s, output drained on separate threads so a full pipe cannot stall them.
Errors are translated (`explain_checkout`, 401/403/404 hints). Checking a PR out is `git fetch <remote> pull/N/head`
then `git checkout -B pr-N FETCH_HEAD`; a dirty tree that would be overwritten is refused by git and explained.
A page of 50 is labelled as the latest 50.

## `.http` client (`http_client.rs`, `workbench_http.rs`)

- Parsing is separate from sending: blocks split at `###`, `@name = value` variables, `# @name` for chaining
  (`{{login.response.body.$.token}}`), `{{$guid}}`, `{{$timestamp}}`, `{{$randomInt}}`, `{{$datetime}}`,
  `{{$processEnv NAME}}`, `< file` bodies.
- `prepare` validates (http/https only, no whitespace in the URL, no line breaks in headers) and `curl_args` builds an
  argument vector; no shell is involved. `curl` runs through `Host::exec` on the workspace host (or locally with
  `http.runOn = "local"`), with `--max-time 60`, `--max-redirs 10` and `--max-filesize` of 20 MB.
- Responses are decoded by `charset` (UTF-8, Latin-1, Turkish Latin-5), otherwise lossily for text types; binary
  bodies open as a hex dump with a truncation note.
- `sensitive_parts` flags requests that read the environment or upload files from outside the `.http` file's folder;
  the user confirms once per file and session. Only one request is in flight at a time.
- `http.confirmRequests` (`never` | `remote` | `always`, default `remote`) is a second reason to ask, answered by the same prompt:
  `schema::gate::HostGate` holds the policy and the hosts approved in the session. The host is taken from the URL after
  `substitute` (so variables are resolved; `host_of` strips user info, port and brackets and returns `None` when a backslash makes
  the authority ambiguous, which always asks). Loopback addresses and `localhost` never ask under `remote`. The gate is shared with
  schema downloads: `Store::text` refuses any host other than SchemaStore until it is approved, records the host once, and the
  workbench asks (`ask_schema_hosts`), then re-checks the open files; a decline is remembered for the session. Redirects that `curl`
  follows are not re-checked.
- Failure messages come from `explain_curl`, which says where the request ran and adds the `http.runOn` hint for
  `localhost` addresses.

## Log follow and large files (`large.rs`, `workbench_panels.rs`, `crates/agent/src/bigfile.rs`)

The large-file view asks the agent for 256-line chunks near the viewport and keeps a bounded number of chunks.
Follow mode polls the agent's incrementally extended line index, detects truncation and rotation (the view says the
file vanished or was replaced), counts unseen lines while the user has scrolled away (badge, `End` to resume), and
colours log levels. `.log` files follow automatically. *Filter Log Lines* runs
`tail -c 268435456 | grep -a -i -E -e PATTERN | cut -c1-2000 | tail -n 5000` on the file's host: only the last
256 MB, at most 5000 lines of at most 2000 characters.

## Other services

- **LSP** (`lsp/`, `workbench_lsp.rs`, `editor_lsp.rs`): own JSON-RPC client; servers run on the workspace host through
  `Host::proc_*`; diagnostics, completion, hover, rename, code actions, inlay hints, semantic tokens, call hierarchy.
- **DAP** (`dap.rs`, `workbench_debug.rs`): debug adapters through the same process interface; launch configurations
  come from `launch.json` (see `debug_config.rs`).
- **Import from another editor** (`vscode.rs`): converts settings and keybindings it understands, reports what it skipped;
  the other editor's files are never modified.
- **EditorConfig, Emmet, linked tags, hex view, snippets**: pure functions with budgets (Emmet: 2000 nodes, 256 KB of
  output; EditorConfig: bounded globs and sections).
