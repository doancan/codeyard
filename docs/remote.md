# Remote development

A remote workspace runs the same code as a local one. The UI talks to a `Host` (`crates/host`); a local host runs
the agent in a thread of the app, a remote host runs it on the server over `ssh` stdio. Everything that touches
files, search, terminals, git and language servers goes through that one interface.

## Connection

- Transport: the system `ssh` with `ControlMaster=auto` and `ControlPersist=600`. Control sockets are named `cm-<16 hex>` (a hash of the host alias and, when set, the absolute `CODEYARD_SSH_CONFIG` path) and live in
  `~/.cache/codeyard`; when that path is too long for a Unix socket (limit 104 bytes on macOS, 108 on Linux, and ssh adds 17 while
  binding) they go to `/tmp/cy-<uid>` (mode 0700). The app refuses that directory if it is a symlink or belongs to another user,
  and then connects without sharing. Control masters opened by the app are closed on exit.
- Scripts the app runs on the server are wrapped in `sh -c`, so they do not depend on the user's login shell.
  Programs (git, language servers) are found through the login shell so the user's `PATH` applies: `exec "$0" "$@"` in
  POSIX shells, `exec $argv` in fish, and `/bin/sh` for any other shell; the program and its arguments are passed as
  arguments, never inside the script.
- Agent install: on first connect the app detects the architecture (`uname -sm`), copies the bundled static agent
  (`codeyard-agent-<triple>`) to `~/.cache/codeyard/agent-v<VERSION>-<sha12>` and verifies its SHA-256 before
  running it. The server never downloads anything, so offline servers work. The version in the path lets agents of
  different protocol versions coexist; `proto::VERSION` is checked in the `Hello` handshake.
- Windows embeds these agents in the EXE and sends the bytes directly over SSH; no separate agent directory,
  local extraction or extra download is needed. Embedded agents take precedence over old sidecar files; an explicit
  `CODEYARD_AGENT_DIR` can override them for development. They ship inside the app's normal signed update.
- `Remote: Test SSH Connection…` reports reachability, architecture and whether a matching agent is bundled.
- Workspaces are identified by both host and folder path. Switching servers resets the editor even when their
  absolute folder paths match, and each server keeps its own saved tabs. Disconnect saves that session and clears
  its folders, Git state, language servers and project settings from the window.

## Authentication prompts

The app acts as `SSH_ASKPASS` (`SSH_ASKPASS_REQUIRE=force`): `ssh` starts the app's own binary in client mode
(`host::askpass`), which forwards the question over a Unix socket in a 0700 directory to an overlay in the window
(masked for passwords and passphrases, yes/no for unknown host keys, free text for one-time codes). The answer is
never written to disk, environment or argv. When no bridge is available the app uses `BatchMode=yes`, so the CLI
and tests can never hang on a prompt.

`crates/app/tests/ssh_askpass.rs` exercises the whole chain against a real `sshd` started by the test (a
passphrase-protected key stands in for a password because a non-root `sshd` cannot verify system passwords).

## Agent and protocol

`crates/proto` defines the messages; frames are length-prefixed `postcard`, requests carry an id, so many calls and
streams are multiplexed. Current protocol version: **14**.

| Group | Requests | Notes |
| --- | --- | --- |
| handshake | `Hello`, `Ping`, `Bye` | versions, OS and architecture, session resume; `Bye` says "closing on purpose" so the daemon ends the session at once instead of waiting out the grace period |
| files | `ReadDir`, `Stat`, `Read`, `Write`, `CreateFile`, `CreateDir`, `Rename`, `Copy`, `Delete`, `ListFiles` | `Write` carries the expected hash and answers `Conflict` when the disk changed; writes are temp file + fsync + rename, keeping mode, owner and symlink target; reads are streamed, whole-file limit `proto::MAX_FILE` (32 MB). Version 13 adds filesystem-native recursive copies, with private staging and exclusive atomic publication; symlinks are preserved, not followed, and copying has no whole-file read limit. Moves also use exclusive native rename operations. Version 14 adds the file identity (device and inode on Unix) and creation time to `Stat`, so Source Control can tell a repository created again at the same path from the one it knew. |
| preview | `ReadPreview` | Version 12 adds one bounded read beneath a root through directory handles. Symlinks, reparse points, hidden paths and special files are refused; the opened file supplies both metadata and content. No per-component SSH requests are needed. Older agents are replaced on reconnect. |
| search | `Search`, `Cancel` | `ignore` + `regex`, results streamed in chunks, cancellable; `Cancel` also interrupts staged copies, which acknowledge cleanup with `Cancelled` |
| watch | `Watch` | polls the given path set once a second; change events are pushed |
| terminals | `PtySpawn`, `PtySpawnEx`, `PtyInput`, `PtyResize`, `PtyKill`, `PtyStatus`, `ListShells` | output is a pushed event; the agent keeps 256 KB of scrollback per pty for resume. `PtySpawnEx` (version 11) takes a profile's shell, arguments and environment and turns on shell integration; `PtyStatus` names the foreground program and the shell's folder (asked before closing a busy terminal and for splits); `ListShells` reads `/etc/shells` for the profile picker |
| processes | `Exec`, `ProcSpawn`, `ProcInput`, `ProcKill` | `Exec` runs a one-shot command with bounded output (git, curl for `.http`, helper scripts); `Proc*` runs long-lived processes (language servers, debug adapters) |
| folders | `HashTree`, `Cancel` | Compare Folders: one pass walks a folder (`.gitignore` and extra globs, symlinks never followed) and streams size, mtime and link target; a second pass hashes only the files the client names. Version 10 added it, so agents from earlier releases are replaced on the next connect |
| big files | `LineIndex`, `ReadLines` | the agent samples a byte offset every 4096 lines and extends the index incrementally; used by the large-file view and log follow |

Compare Folders works with the window's one connection: a side is a folder on this computer or on the connected host. Comparing two
SSH hosts would need a second connection per window and is left for a later version.

Git and language servers are **not** agent features: the agent only runs processes, and parsing, diffing and
JSON-RPC are in the app. That keeps the agent small and gives local and remote exactly one code path. Port
forwarding does not go through the agent either: it adds `ssh -O forward` rules to the existing control master; the
listening ports are found by `Exec`-ing `ss`, a read of `/proc/net/tcp*` or `lsof`.

### Port forwarding

- **Target address.** Each listening port is read with the address it is bound to. `127.0.0.1` and `::1` are forwarded to
  exactly that address, a wildcard bind to `localhost`, and any other address to itself, so a service bound to only one
  loopback address is reached.
- **Local side.** Before a forward is added, the local port is tried on both `127.0.0.1` and `::1`: a port another program
  holds on either one is reported busy instead of silently reaching that program. `ports.localBind` (user settings only)
  decides between loopback and every interface.
- **One owner of the state.** Forwards are kept by a small state machine (`port_forwards::ForwardSet`). Every change (a
  click, a detection, a reconnect, a disconnect) returns the `ssh -O forward` / `-O cancel` effects to run, in order, on
  a background thread. Opening the same port twice is impossible; a stale rule is cancelled before its port is reused; a
  dropped connection cancels its rules; and forwards come back on the same local port after a reconnect.
- **Persistence.** Forwards made by hand are saved per project (label, protocol, local port) in the state database and
  added again when the project is opened over SSH.

## Robustness rules the agent keeps

- **Concurrency.** Streaming work (search, `Exec`, large-file index, terminals, processes) runs on its own threads and file
  operations (`ReadDir`, `Read`, `Write`, `Delete`, `ListFiles`, …) on a pool of 4 workers, so a slow directory walk cannot
  delay `Cancel`, `Watch` or keystrokes; answers carry their request id. Input to terminals and processes goes through a bounded
  queue and a writer thread (a stopped language server with a full pipe returns `WouldBlock` instead of freezing the session).
  The host side gives every call a deadline (2 min; 15 min for `Exec` and large-file scans) and reports a clear error.
- **Heartbeat.** Every 15 s the host sends `Ping` (answered by the dispatch thread itself, independent of the worker pool and
  of running processes). Two misses in a row (20 s each) drop the ssh process, which starts the normal reconnect; ssh's own
  keep-alive catches dead networks, this catches a hung agent behind a healthy one.
- **Daemon start-up** is serialised by an `flock` on `<socket>.lock`, so two bridges starting together cannot delete each other's
  fresh socket.
- **Saving** writes a new file exclusively (`O_EXCL`, unpredictable name, so a planted symlink is refused, never followed);
  an existing file's content is only ever in a temp file readable by its owner (0600) until the final permissions are applied;
  long names are shortened so the temp name cannot exceed `NAME_MAX`.
- `Read`, `LineIndex` and `ReadLines` accept regular files only (a FIFO or `/dev/zero` would block the one dispatch thread) and
  never read more than `MAX_FILE` bytes even if the file grows after the size check.
- `Delete` refuses the root, the home folder **and its parents** (`/home`, `/Users`).
- `Exec` and `ProcSpawn` fail when the given working directory does not exist; they never fall back to the agent's own folder.
- A response larger than one frame (`MAX_FRAME`, 64 MB) becomes an error for that request; the link stays up. Reconnect
  replays buffered pty output, exits and file changes only after they were written successfully.
- A killed pty child is reaped (no zombie shells); a failed handshake kills and waits for its ssh process, and the handshake
  has a 30 s deadline so a silent agent cannot stall reconnecting; a window closed during a reconnect never keeps the new
  connection.
- Long-lived processes (`ProcSpawn`) lead their own process group on Unix; `ProcKill` and a dropped connection kill the
  whole group while its leader runs, so a stopped test run or debug session leaves no test binary or debuggee behind.
- Shell-integration scripts are written to `$XDG_CACHE_HOME/codeyard/shell-integration/<hash>` (or `~/.cache/…`), in 0700
  folders with 0600 files. A folder that is a symlink or belongs to another user is refused, and a changed script is
  rewritten before use.
- tar operands are `./name` (download) or `.` inside a resolved `-C` directory (upload), so a folder called `-x…` is never read as an option.

## Surviving a dropped connection

The agent can run as a daemon on the server (`--daemon SOCK`, socket mode 0600 in a 0700 directory). Each ssh
connection then only runs a bridge (`--attach SOCK`). When ssh drops, the daemon keeps the session's ptys and
watches for `GRACE` (5 minutes) and exits after `IDLE_EXIT` (10 minutes) with no sessions. The client reconnects with
a 1 s → 15 s back-off, sends `Hello{session}` and gets `resumed: true`. Long-lived processes are tied to their
connection: they are killed when it drops and the client restarts them, because a lost message would corrupt the
stream. If reconnecting fails the user sees the reason and a Reconnect button; open buffers and unsaved changes stay.
`CODEYARD_RECONNECT_WINDOW_SECS`, `CODEYARD_DAEMON_GRACE` and `CODEYARD_DAEMON_IDLE` shorten the windows for tests.

## `~/.ssh/config` editing (`host::ssh_config`)

Add, edit and remove hosts without touching anything else in the file:

- every write keeps the previous file as `config.codeyard-bak` and replaces the file atomically;
- a symlinked config is edited through the link (the symlink stays), CRLF files stay CRLF;
- values with spaces are quoted (`IdentityFile "…"`), duplicate aliases are rejected case-insensitively;
- a host defined in an `Include`d file is reported as such instead of being edited blindly;
- read-only and missing files produce explanations, not panics.

`Remote: Set Up Key Login…` creates `~/.ssh/id_ed25519` when there is no key, then appends the public key to the
server's `authorized_keys` (newline-safe, skipped when the same key blob is already present), asking for the password
through the askpass bridge.

## File transfer (`host::transfer`)

Uploads and downloads are streamed over the shell: single files via `cat` into `<name>.codeyard-part.$$` and `mv`
when complete, folders via `tar -cf -` | `tar -xof -` (only flags that bsdtar, GNU tar and busybox share; a top-level symlink is resolved by the client). Properties the tests lock in:

- a file and a folder of the same name never replace each other (detected before anything is copied);
- cancelling or failing removes only what this transfer created: a new folder is cleaned up, a folder that already
  existed (locally or on the server) is never deleted;
- a source file that cannot be read fails the whole transfer with the reason and leaves no half-copied new folder;
- a dropped connection reports what was and was not copied;
- progress is reported per chunk and the cancel flag is checked between chunks.

## Dev containers and WSL

`Remote: Connect to Dev Container…` and `…WSL Distro…` use the same agent over `docker exec` / `wsl.exe`. Port
forwarding stays SSH-only. The WSL path has only been unit-tested (output parsers); it has not been run on Windows.
