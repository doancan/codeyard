# Environment variables

All variables are optional. `scripts/docs-check.sh` fails when the code reads a `CODEYARD_*` variable that is not listed here.

## For users

| Variable | Effect |
| --- | --- |
| `CODEYARD_STATE_DIR` | Folder for the local state database, crash reports and backups (default: `~/Library/Application Support/codeyard` on macOS, `~/.local/state/codeyard` elsewhere). Local managed language services also use this profile’s `language-services` subdirectory. |
| `CODEYARD_CONFIG_DIR` | Folder for `settings.jsonc`, `keybindings.jsonc`, `themes/`, `snippets/`, `projects.jsonc` (default `~/.config/codeyard`). |
| `CODEYARD_SSH_CONFIG` | Use this file instead of `~/.ssh/config` for host lists and connections. |
| `CODEYARD_AGENT_DIR` | Optional developer override for `codeyard-agent-<triple>` binaries. Windows uses embedded agents by default; Unix packages use `agents/` next to the executable (or the macOS bundle's Resources directory). |
| `CODEYARD_REMOTE_DIR` | Where the agent is installed on the server (default `~/.cache/codeyard`). |
| `CODEYARD_NO_A11Y` | Set to disable the accessibility tree (macOS and Windows). |
| `CODEYARD_NO_DOCK_ICON` | Do not set the Dock icon at run time (bare binaries only). |
| `CODEYARD_DAP_TRACE` | Print the debug adapter protocol traffic to stderr. |
| `CODEYARD_LSP_TRACE` | Print language server messages to stderr. |
| `CODEYARD_JS_DEBUG` | Folder of an existing Microsoft js-debug installation. |
| `CODEYARD_RECONNECT_WINDOW_SECS` | How long the app keeps trying to reconnect after a dropped SSH connection (default 300). |

## For development and tests

| Variable | Effect |
| --- | --- |
| `CODEYARD_UPDATE_URL` | Replaces `<repository>/releases/latest/download` as the place update files are fetched from (a mirror, or a local `http://` server or `file://` folder to try the updater). Only the download location changes: the signature check with the built-in keys still applies. |
| `CODEYARD_DEV_UPDATE` | Developer builds: play a whole update with canned data instead of asking GitHub: `available`, `uptodate`, `offline`, `untrusted` or `install-fails`. The download crawls at 1 MB/s, nothing touches the disk, and quitting only prints a line. Also lets a debug build show the first-run question about update checks, which it never does otherwise. |
| `CODEYARD_DEV` | Enable developer commands in a release build (always on in debug builds). Without it, release builds ignore `--do`, because a command line another program put together could otherwise trust a folder and run its tasks. |
| `CODEYARD_MENU_FLAVOR` | `macos`, `windows` or `linux`: draw menus in that platform's style instead of the running one (to review the other platforms' look). |
| `CODEYARD_REGISTRY` | The registry `cyx publish`, `yank`, `restore` and `deprecate` talk to (default: the public registry); `--registry` wins. |
| `CODEYARD_FULCIO_URL`, `CODEYARD_REKOR_URL`, `CODEYARD_OIDC_URL` | Where `cyx publish` gets its certificate, logs its signature and signs in (default: public-good Sigstore). For a private Sigstore; the registry must pin the same roots. |
| `CODEYARD_BIN` | The Codeyard binary `cyx dev` starts (default: `codeyard` on `PATH`, then the one beside `cyx`, then `/Applications/Codeyard.app` on macOS). |
| `CODEYARD_EXTHOST` | The `codeyard-exthost` that `cyx test` runs extensions in (default: the one beside `cyx`, then on `PATH`). |
| `CODEYARD_CYX_CACHE` | Where `cyx` keeps what it installs and reuses: the TypeScript build tools (esbuild, ComponentizeJS, installed with `npm ci`), the TypeScript SDK it bundles, and the extension host's incremental compile cache (default: `~/Library/Caches/codeyard/cyx` on macOS, `$XDG_CACHE_HOME/codeyard/cyx` or `~/.cache/codeyard/cyx` on Linux, `%LOCALAPPDATA%\codeyard\cyx` on Windows). |
| `CODEYARD_BUILD_ID` | Build time: the id the app and `codeyard-exthost` compare when one starts the other (default: the crate version). A release build sets one per build so an app updated in place restarts its extension hosts. |
| `CODEYARD_PERF` | Append performance samples (one JSON line each) to this file; used by `scripts/perf/run.sh`. |
| `CODEYARD_PERF_TYPING` | With `CODEYARD_PERF`: after startup, type this many characters into the focused editor, record key press to drawn frame for each, then save. |
| `CODEYARD_PERF_MARKET` | With `CODEYARD_PERF`: after startup, open the Market, wait for its list, then press this many keys in its search (a word typed and erased), recording key press to the results in the view and to the frame that shows them. |
| `CODEYARD_PERF_EXIT` | With `CODEYARD_PERF`: quit when the scripted scenario is done. |
| `CODEYARD_DO_DELAY_MS` | Delay before `--do "Command;Command"` runs its commands. |
| `CODEYARD_DUMP_DOCK` | Print the Dock menu to stderr a few seconds after start; with a number, click that row. |
| `CODEYARD_DAEMON_GRACE`, `CODEYARD_DAEMON_IDLE` | Seconds the server-side agent daemon waits for a lost client and before exiting with no sessions (defaults 300 and 600). |
| `CODEYARD_SHELL_INTEGRATION`, `CODEYARD_USER_ZDOTDIR` | Internal: set in a terminal's shell when shell integration is on, and (for zsh) where the user's own startup files are, so the integration scripts read them first. A profile's `env` may set `CODEYARD_USER_ZDOTDIR`. |
| `CODEYARD_ASKPASS_EXE`, `CODEYARD_ASKPASS_SOCK` | Internal: the executable `ssh` runs as `SSH_ASKPASS` and the socket it answers on. Tests point the first at a test binary. |
| `CODEYARD_QA_DIR`, `CODEYARD_QA_TMP` | Base folders for test scratch files. |
| `CODEYARD_QA_SEEDS`, `CODEYARD_QA_SEED_BASE` | Number of random sessions and the first seed of `qa_random_new_commands_many_seeds`. |
| `CODEYARD_REQUIRE_SSHD` | Set to `1` to make the SSH integration tests fail instead of skipping when no usable `sshd` or agent binary is found. Without it a skipped test prints a `SKIPPED` line and passes. CI sets it on the jobs that ship `sshd`. |
| `CODEYARD_REQUIRE_WASM` | Set to `1` to make the tests that build code extensions (`cyx` tests of `examples/extensions/`) fail instead of skipping when the `wasm32-wasip2` target, Node.js 22 or newer, or a built `codeyard-exthost` is missing. CI sets it on the Linux job, the only one with that target and Node.js. |
| `CODEYARD_REQUIRE_TOOLS` | Same for tests that need an external tool (python3, node, clang, lldb-dap, debugpy, go, dlv, clangd): `1` for all of them, or a comma-separated list such as `python3,clang`. |
| `CODEYARD_FISH` | Path of a fish binary that is not on `PATH`, so the quoting and login-shell tests also run against fish. |
| `CODEYARD_FUZZ_SECS` | Make the hostile-input (`fuzz_tests.rs`) tests of the extension crates keep going for this many seconds instead of a fixed number of rounds; set by `.github/workflows/fuzz.yml`. |
| `CODEYARD_FUZZ_SEED` | Change the seed of those tests to explore other inputs; a failure prints the seed and round to rerun. |
| `CODEYARD_TEST_TIMEOUT_SCALE` | Multiplier for the tests' wait timeouts (default 1). `scripts/coverage.sh` sets 4 because instrumented builds are slow. |
| `CODEYARD_TR_DICT_DIR` | Folder with a Turkish Hunspell dictionary for the manual `cargo test -p spell -- --ignored turkish` check. |
| `CODEYARD_LANGUAGE_SERVICE_FIXTURE` | Explicit disposable managed-service installation for the opt-in real local/SSH language-service acceptance test. Ordinary tests do not read it. |
| `CODEYARD_TEST_MANAGED_INSTALL` | With that opt-in test, `1` additionally installs packages/runtime under its owned SSH fixture; this can access the network. |
