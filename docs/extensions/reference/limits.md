# Limits

<!-- Generated from crates/ext-manifest/src/limits.rs: run `UPDATE_DOCS=1 cargo test -p ext-manifest reference` after changing it. -->

Every size, count and time limit of the extension platform. `cyx check` and the registry refuse a manifest or package over a limit with the error that names it; at run time a call over a limit fails, and an extension host over a memory, time or traffic limit is stopped. The names are the constants in `crates/ext-manifest/src/limits.rs`, which other docs also use.

## Manifest

| Limit | Value | What it covers |
| --- | --- | --- |
| `MANIFEST_BYTES` | 64 KiB | `codeyard-extension.toml` larger than this is refused before it is parsed. |
| `ID_LEN` | 64 characters | `publisher.name` as a whole. |
| `ID_PART_LEN` | 40 characters | One part of an id (publisher, name, or a contribution's local id). |
| `NAME_LEN` | 64 characters | `extension.name`, the display name. |
| `DESCRIPTION_LEN` | 300 characters | `extension.description`. |
| `WHY_LEN` | 200 characters | The author's `why`, quoted on the consent sheet. |
| `CATEGORIES` | 5 | Entries of `extension.categories`. |
| `CONTRIBUTIONS_PER_KIND` | 256 | Entries of one contribution kind (`[[commands]]`, `[[themes]]`, …). |
| `PERMISSION_ENTRIES` | 64 | Entries of one permission list (`read`, `net`, `run`, …). |
| `DEPENDENCIES` | 32 | Entries of `[dependencies]`. |
| `ITEM_LEN` | 256 characters | Length of a glob, host, program, path or title. |
| `WHEN_LEN` | 512 characters | Length of a `when` / `enablement` expression. |
| `WHEN_DEPTH` | 16 | Nesting depth of a `when` expression (parentheses and `!`). |

## Packages and storage

| Limit | Value | What it covers |
| --- | --- | --- |
| `PACKAGE_BYTES` | 64 MiB | A `.cyx` package, compressed. |
| `TARGET_PACKAGE_BYTES` | 128 MiB | A per-platform package (`<id>-<version>-<triple>.cyx`, one that carries native programs), compressed. |
| `PACKAGE_UNPACKED_BYTES` | 256 MiB | A `.cyx` package, unpacked. |
| `PACKAGE_FILES` | 10,000 | Files in a `.cyx` package. |
| `PACKAGE_FILE_BYTES` | 64 MiB | One file inside a package. |
| `DATA_DIR_BYTES` | 50 MiB | An extension's own data directory (`storage`). |

## Calls and the extension host

| Limit | Value | What it covers |
| --- | --- | --- |
| `CALL_BYTES` | 8 MiB | The arguments or result of one API call (a file read, a fetch's request body, a view's tree). |
| `FRAME_BYTES` | 8,256 KiB | One frame between the app and an exthost: a call plus room for its envelope. A larger frame kills the exthost. |
| `IPC_BYTES_PER_SECOND` | 32 MiB/s | Sustained bytes per second an exthost may send the app; going over kills it. |
| `IPC_BURST_BYTES` | 64 MiB | Bytes an exthost may send in a burst above the sustained rate. |
| `TIMER_MIN_SECONDS` | 30 s | The shortest interval of `timers.every`. |
| `GUEST_MEMORY_BYTES` | 256 MiB | Linear memory of one extension instance; growing past it traps. |
| `EXTHOST_MEMORY_BYTES` | 512 MiB | The whole exthost process: the guest's linear memory plus the engine and compiled code. A Job Object commit limit on Windows; elsewhere the app checks the process's resident size. |
| `GUEST_CALL_MILLIS` | 2 s | CPU time one call into an extension may take (exports must return quickly; long work waits on tickets). |
| `ACTIVATE_MILLIS` | 10 s | `activate` may take longer: language runtimes start there. |
| `PROVIDER_DEADLINE_MILLIS` | 1 s | How long the app waits for a provider (formatter, code actions, tasks) to answer a request. A later answer is dropped and noted in the extension's log; the user's typing never waits for it. |
| `PULL_REQUEST_DEADLINE_MILLIS` | 20 s | How long the Pull Requests view waits for a `[[pull-requests]]` provider, whose answers come from the network. A later answer is dropped like any provider's. |
| `STATUS_TEXT_LEN` | 64 characters | Text of a status bar item. |
| `MARKS_PER_FILE` | 1,000 | Diagnostics or decorations one extension may set on one file; the rest are dropped. |
| `NET_TIMEOUT_SECS` | 60 s | One `net` request, from the first connection to the last byte of the response, redirects included. |
| `NET_REDIRECTS` | 5 | Redirects one `net` request follows; each is checked against `net` and the peer address rules again. |
| `NET_REQUEST_BYTES` | 16 MiB | The body one `net` request sends. |
| `NET_RESPONSE_BYTES` | 64 MiB | The body one `net` request receives. |

## Views

| Limit | Value | What it covers |
| --- | --- | --- |
| `UI_VIEW_BYTES` | 8 MiB | An encoded view (`ext-ui`), refused before it is decoded. |
| `UI_NODES` | 50,000 | Elements in one view, counting every row and cell. |
| `UI_DEPTH` | 32 | Nesting depth of a view; the root is depth 1. |
| `UI_ROWS_PER_PAGE` | 10,000 | Rows of a list or table, and items in one page of a tree level. |
| `UI_LABEL_LEN` | 256 characters | A label, heading, badge, option or column header. |
| `UI_TEXT_LEN` | 4,096 characters | Body text: `text`, `banner`, a key-value value, an input's value or placeholder. |
| `UI_DOCUMENT_BYTES` | 64 KiB | The source of one `markdown` or `code` element, in bytes. |
| `UI_KEY_LEN` | 128 characters | An element key or a tree page token. |
| `UI_IMAGE_BYTES` | 1 MiB | An `image` given as bytes. |
| `UI_IMAGE_URLS` | 64 | Different `image` URLs one open view fetches; the rest show their alt text. |
| `UI_CHOICES` | 256 | Options of a `select`, pairs of a `key-value`. |
| `UI_COLUMNS` | 32 | Columns of a `table`. |
| `UI_URL_LEN` | 2,048 characters | An `https:` URL of an image or link. |

## Source control

| Limit | Value | What it covers |
| --- | --- | --- |
| `SCM_MARKERS` | 8 | Markers of one `[[scm]]` provider. |
| `SCM_RESOURCES` | 10,000 | Files one source control provider may list for one folder, over all its groups; the rest are dropped. |
| `SCM_ORIGINAL_BYTES` | 8 MiB | The text of a file a source control provider returns for the gutter's change marks; a longer one shows no marks. |
| `SCM_DEADLINE_MILLIS` | 10 s | How long the app waits for a source control provider's state, a file's original text or a check of the input box. Longer than other providers: they usually run a program. A later answer is dropped and noted in the log. |
