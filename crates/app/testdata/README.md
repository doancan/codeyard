# Test data

Real output of the tools Codeyard talks to, kept so parsers are tested against what the tools print and not against what we
expect them to print. Paths of the machine they were captured on are replaced by `/work/demo` (and `/toolchain`).

## Tools and versions

| Tool | Version |
| --- | --- |
| rust-analyzer | 0.3.3113-standalone (e5e7830640 2026-11-08), the official release binary |
| cargo-nextest | 0.9.151 (6b3f99457 2026-11-02) |
| rustc / cargo | 1.99.1 (58cf8f9c3 2026-10-13) / 1.99.1 (5f94df478 2026-08-27), Homebrew, aarch64-apple-darwin |

## The package

`demo`: `src/lib.rs` (`add`, `unsafe_read` with an `unsafe` block, `mod tests` with `adds_up`, `fails_on_purpose`, and
`ignored_one` marked `#[ignore = "slow"]`), `src/main.rs` (`main` and `tests::main_is_fine`) and `tests/integration.rs`
(`adds_from_outside`, and `panics_with_unicode`, which panics with `ğüşıöç 😀`). `tests::fails_on_purpose` compares 3 with 4.

## `matchers/`

Output of real compilers and interpreters for the per-task problem matchers (`task_matchers_tests.rs`): `clang-errors.txt`
(Apple clang 21.0.0, `clang -Wall -c şablon.c`: two errors and a warning in a file with a Turkish name), `cargo-errors.txt`
(`cargo build --color never` on a two-line program with two errors; the project path is replaced by `/work/t`) and
`python-traceback.txt` (Python 3.9.6 `ValueError`). The tests also feed them with CRLF line ends and in tiny chunks.
tsc and eslint were not installed on the capture machine, so their matchers are covered by the engine's synthetic tests only.

## `rust-analyzer/`

Captured by starting `rust-analyzer` on `demo` as a language server (stdio), initializing with the client capabilities
`window.workDoneProgress`, `workspace.configuration`, `experimental.serverStatusNotification` and
`textDocument.publishDiagnostics.codeDescriptionSupport`, opening both files, waiting for `experimental/serverStatus` to report
`quiescent: true`, then sending the requests below with a position inside the item. Answers to the server's own requests were
`null`.

| File | Request or notification |
| --- | --- |
| `runnables-lib.json`, `runnables-main.json` | `experimental/runnables` with `position: null` for `src/lib.rs` and `src/main.rs` |
| `view-hir.json`, `view-mir.json` | `rust-analyzer/viewHir`, `rust-analyzer/viewMir` at `add` |
| `syntax-tree.json`, `syntax-tree-source.rs` | `rust-analyzer/viewSyntaxTree` (`range: null`) for a file of 116 bytes, and that file |
| `external-docs.json` | `experimental/externalDocs` at `println!` |
| `semantic-tokens.json` | the server's token type and modifier tables and the first 200 numbers of `textDocument/semanticTokens/full` for `src/lib.rs` |
| `notifications.jsonl` | every `experimental/serverStatus` and flycheck `$/progress` (`rust-analyzer/flycheck/0`) notification of a session that ran `rust-analyzer/runFlycheck` twice and `rust-analyzer/reloadWorkspace` once |

## `nextest/`

Run in `demo` with `cargo-nextest nextest run …` (stdout and stderr together, no terminal, so no colour) and
`cargo test …`:

| File | Command |
| --- | --- |
| `all.txt` | `cargo nextest run --no-fail-fast --run-ignored all` |
| `skip.txt` | `cargo nextest run --no-fail-fast --status-level skip` |
| `pass.txt` | `cargo nextest run --no-fail-fast adds_up` |
| `fail.txt` | `cargo nextest run` (stops at the first failure) |
| `libtest-all.txt` | `cargo test --no-fail-fast -- --include-ignored` |
| `libtest-default.txt` | `cargo test --no-fail-fast` |

`FLAKY`, `TIMEOUT` and signal lines are not in these files (nothing in `demo` retries or times out); the tests for them use
lines written by hand from nextest's documentation and say so.

## `refactor/`

A second package, `refactor/demo/src/`, with a symbol used in four files: `hesapla` in `util.rs`, called from `main.rs`,
`ayar.rs` (after Turkish letters and an emoji on the same line, so the UTF-16 columns matter) and `rapor.rs` (CRLF on disk;
the fixture here has LF so checkouts do not change it, and the tests put the CRLF back). Captured with rust-analyzer
0.3.2887-standalone (f04c372864 2026-05-03, the official release binary), client capabilities
`workspace.workspaceEdit.documentChanges` and `resourceOperations: [create, rename, delete]`, the files opened with version 1
and the answer waited for until the server was idle. The folder of the machine it was captured on is replaced by `/ws/demo`.

| File | Request |
| --- | --- |
| `rename_fn.json` | `textDocument/rename` of `hesapla` to `topla` at `util.rs` 0:7 (`util.rs` is open, so its edit has version 1; the others are `null`) |
| `rename_mod.json` | `textDocument/rename` of the module `util` to `araçlar` at `main.rs` 0:4: text edits in three files and a `rename` of `util.rs` to `araçlar.rs` |

Create and delete operations are not in these captures (rust-analyzer sent none for these requests); their tests use edits
written by hand, in the shape of the protocol.

## Other

`rustc-explain-E0382.md`: `rustc --explain E0382`. Output of `rustup target list --installed` was not captured: rustup was not
installed on the machine; the test for it uses a sample written by hand.
