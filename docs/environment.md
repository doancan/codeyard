# Environment variables

All variables are optional. `scripts/docs-check.sh` fails when the code reads a `CODEYARD_*` variable that is not listed here.

## For users

| Variable | Effect |
| --- | --- |
| `CODEYARD_STATE_DIR` | Folder for the local state database, crash reports and backups (default: `~/Library/Application Support/codeyard` on macOS, `~/.local/state/codeyard` elsewhere). |
| `CODEYARD_CONFIG_DIR` | Folder for `settings.jsonc`, `keybindings.jsonc`, `themes/`, `snippets/`, `projects.jsonc` (default `~/.config/codeyard`). |
| `CODEYARD_SSH_CONFIG` | Use this file instead of `~/.ssh/config` for host lists and connections. |
| `CODEYARD_AGENT_DIR` | Optional developer override for `codeyard-agent-<triple>` binaries. Windows uses embedded agents by default; Unix packages use `agents/` next to the executable (or the macOS bundle's Resources directory). |
| `CODEYARD_REMOTE_DIR` | Where the agent is installed on the server (default `~/.cache/codeyard`). |
| `CODEYARD_NO_A11Y` | Set to disable the macOS accessibility tree. |
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
| `CODEYARD_DO_DELAY_MS` | Delay before `--do "Command;Command"` runs its commands. |
| `CODEYARD_DUMP_DOCK` | Print the Dock menu to stderr a few seconds after start; with a number, click that row. |
| `CODEYARD_DAEMON_GRACE`, `CODEYARD_DAEMON_IDLE` | Seconds the server-side agent daemon waits for a lost client and before exiting with no sessions (defaults 300 and 600). |
| `CODEYARD_SHELL_INTEGRATION`, `CODEYARD_USER_ZDOTDIR` | Internal: set in a terminal's shell when shell integration is on, and (for zsh) where the user's own startup files are, so the integration scripts read them first. A profile's `env` may set `CODEYARD_USER_ZDOTDIR`. |
| `CODEYARD_ASKPASS_EXE`, `CODEYARD_ASKPASS_SOCK` | Internal: the executable `ssh` runs as `SSH_ASKPASS` and the socket it answers on. Tests point the first at a test binary. |
| `CODEYARD_QA_DIR`, `CODEYARD_QA_TMP` | Base folders for test scratch files. |
| `CODEYARD_QA_SEEDS`, `CODEYARD_QA_SEED_BASE` | Number of random sessions and the first seed of `qa_random_new_commands_many_seeds`. |
| `CODEYARD_REQUIRE_SSHD` | Set to `1` to make the SSH integration tests fail instead of skipping when no usable `sshd` or agent binary is found. Without it a skipped test prints a `SKIPPED` line and passes. CI sets it on the jobs that ship `sshd`. |
| `CODEYARD_REQUIRE_TOOLS` | Same for tests that need an external tool (python3, node, clang, lldb-dap, debugpy, go, dlv, clangd): `1` for all of them, or a comma-separated list such as `python3,clang`. |
| `CODEYARD_TEST_TIMEOUT_SCALE` | Multiplier for the tests' wait timeouts (default 1). `scripts/coverage.sh` sets 4 because instrumented builds are slow. |
| `CODEYARD_TR_DICT_DIR` | Folder with a Turkish Hunspell dictionary for the manual `cargo test -p spell -- --ignored turkish` check. |
