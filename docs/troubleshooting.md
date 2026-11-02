# Troubleshooting

Short answers to the problems people actually hit. Messages are quoted as the app shows them.

## The app does not open or bounces in the Dock (macOS)

macOS guards Desktop, Documents, Downloads, iCloud Drive and external drives. Codeyard asks for nothing at start; the first time you open a folder
in one of them the system asks in its own dialog: answer **Allow**. Full Disk Access is optional (it also covers `~/Library` and every drive) and macOS
never lists an app there by itself: run *Help: Grant Full Disk Access…*, press **+** in the page that opens (or drag Codeyard in from Finder) and turn it on.
Release builds are signed with Developer ID and notarized by Apple, so they open normally on first launch. Older releases
from before signing count as a new app to macOS after each update and need **right-click ▸ Open** on first launch (or
`xattr -cr /Applications/Codeyard.app`); if an old entry stays listed but does not work, remove it with **−** and add it again.
Windows builds are not code-signed, so SmartScreen may warn on first launch.

## SSH

| What you see | What it means and what to do |
| --- | --- |
| a prompt for a password, passphrase or host key | `ssh` asked and the app forwarded the question; answer it in the window. Nothing is stored. |
| "no agent binary bundled for `<triple>`" | The server's architecture has no matching `codeyard-agent-<triple>` next to the app. Only Linux x86_64 / aarch64 and macOS arm64 are bundled. Use **Remote: Test SSH Connection…** to see the detected architecture. When running the bare binary, keep its `agents/` folder beside it (or set `CODEYARD_AGENT_DIR`). |
| "could not start ssh" | `ssh` is not on the app's `PATH`. macOS and Linux ship it; on Windows install OpenSSH. |
| *Reconnecting* in the status bar | The connection dropped; the agent keeps your terminals and watches alive for 5 minutes. If it gives up you get the reason and a **Reconnect** button; open files and unsaved edits stay in their tabs. |
| *Permission denied (publickey)* | Add the key to the server (**Remote: Set Up Key Login…**) or point the host at the right `IdentityFile` (**Remote: Edit SSH Host…**). |
| An upload or download stopped | A cancelled or failed transfer only removes what it created. Existing folders are never deleted. |
| `ControlPath too long` | The app keeps its control sockets in `/tmp/cy-<uid>` instead of the home folder; if that directory is unsafe it connects without sharing. |

## Language servers

**New folder, no completions or errors?** Language servers are off in folders you have not trusted yet: they can run the
project's build scripts. Run *Workspace: Trust This Folder* (or start any task and answer the prompt); they start at once.

The status bar says once "`<server>` isn't available" (with an install hint on a remote folder: "on `<host>`") and nothing else breaks. Install it (the table in the user guide's *Language
servers* section has one command per language) and run **Developer: Restart Language Servers**. Servers on a remote folder must be
installed **on the server**. Change or disable one with `lsp.servers`; it is ignored in a folder you have not trusted.

## Schemas, spelling and `.http`

- No JSON/YAML hints: check `json.validate.enable`; for remote or offline machines set a local schema (`$schema` with a relative
  path is read from the workspace host) and `json.schemaDownload.enable: false`. **Schemas: Clear Download Cache** forgets the
  cached copies (kept for 7 days).
- A spell dictionary is missing or unreadable: you get one notice; reinstall it with **Spelling: Install Dictionary…**.
- An `.http` request on `localhost` fails over SSH: the request is sent *from the server*, so `localhost` means the server. Set
  `http.runOn` to `local` to send from your own computer. The message names where the request ran.
- "`curl` is not installed on …": install curl on that machine or switch `http.runOn`.

## GitHub pull requests

`Pull Requests: Show…` uses `gh` when it is installed and signed in, otherwise the public API (rate limited, no private
repositories): run `gh auth login` or set `GITHUB_TOKEN`. A checkout that refuses because of local changes tells you to commit or
stash first.

## Updates

| What you see | What it means and what to do |
| --- | --- |
| "Couldn't check for updates" / "Couldn't download the update" | Codeyard could not reach GitHub (offline, a proxy, a firewall); the line under the message is `curl`'s own. **Try Again**, or take the installer from **Release Page**. A download can also fail because a newer release replaced the one it was fetching: check again. |
| "The update couldn't be verified" | Nothing was installed. What came from the server did not match the signed checksums (a captured connection, a broken mirror) and was discarded. Do not retry blindly: download from the release page and compare with `scripts/verify-release.sh` (see [SECURITY.md](../SECURITY.md)). |
| "The update couldn't be verified" and "This release isn't signed" | The latest release has a checksum list but no `SHA256SUMS.txt.sig` (v0.1.0 is such a release), so Codeyard cannot check it and will not use it. Nothing is wrong with your connection; get the release from its page if you trust it. |
| "Couldn't check for updates" and "no release with a checksum list" | GitHub has no release with a `SHA256SUMS.txt` yet, so there is nothing to compare with. Trying again does not help; look at the release page. |
| "Couldn't check for updates" and "date and time are right" | The secure connection failed (certificate or handshake). Check the date and time of the computer, a proxy or security software that inspects connections, and try again. |
| "There isn't enough free disk space for the download" | The installer is downloaded to Codeyard's state folder; free some space and **Try Again**. |
| "The update failed part-way … may be at …" | The new app could not be put in place and the old one could not be moved back. The message names the folder with the old copy (`.Codeyard.old-…`); if Codeyard does not start, move it back to its place. |
| "The update couldn't be installed" and "cannot write to /Applications" | The app is in a folder your account cannot change. Install by hand from the release page, or move Codeyard to a folder you own. |
| The red "Update failed" item in the status bar | Click it to read the reason; closing that dialog removes the item. |
| **Download Page** instead of *Install and Restart* | This copy cannot replace itself (debug build, bare binary, Linux, Windows ZIP, read-only location); the dialog gives the reason. Download the new version and replace the old one yourself. |
| "Update installed" but the old version still runs | Restart: the dialog's **Restart Now** quits and opens the new version; *Later* only postpones it. |
| You were never asked about update checks | Debug builds and windows started with `--do`/`--panel` never ask; closing the question with Esc snoozes it for 30 days. Set `"update.check"` yourself or use *Help: Check for Updates…*. |

## Extensions

| What you see | What it means and what to do |
| --- | --- |
| "… contains code, which needs a sandbox this system does not provide: AppContainer has not passed the hardware check yet …" (Windows) | Extensions with code do not run on Windows in this version. Extensions without code (themes, languages, snippets, keymaps) work. |
| An extension cannot be found in the Market or by *Install from Registry…* | The public registry is not online yet. Install from a `.cyx` file, or add your own registry to `extensions.registries`. |
| Codeyard misbehaves after installing an extension | Start it with `codeyard --no-extensions` (safe mode: no extension is loaded), then turn the extension off or uninstall it in the Extensions view. |
| An extension you are writing fails | See the [developer guide's troubleshooting](extensions/developer-guide.md#troubleshooting) and `cyx explain <CODE>`. |

## Reporting a problem

If Codeyard panics it writes a report to the `crashes/` folder next to its state database (see `CODEYARD_STATE_DIR` in
[environment.md](environment.md)); nothing is sent anywhere, attach the file to your issue yourself. Include the version
(**About Codeyard** in the gear menu), your OS and what you did. For vulnerabilities see [SECURITY.md](../SECURITY.md).
