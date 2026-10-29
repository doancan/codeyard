# Security policy

## Supported versions

Only the latest release receives security fixes.

## Reporting a vulnerability

Please do **not** open a public issue. Use GitHub's private reporting:
<https://github.com/doancan/codeyard/security/advisories/new>. Include the version, what you did, what you expected and what
happened; a minimal file or configuration that reproduces it helps most.

You can expect an acknowledgement within a few days and a fix or a clear answer as soon as the problem is understood.

## What counts

Codeyard runs programs on your behalf (terminals, tasks, language servers, debuggers, `ssh`, `git`, `curl`), so the areas that
matter most are:

- opening an untrusted folder or file causing code to run without the Workspace Trust prompt;
- the SSH agent bridge, askpass socket, file transfer and `~/.ssh/config` editing;
- the `.http` client sending files or environment variables the user did not approve;
- schema, spell and parser code handling hostile input (crashes, unbounded memory or time).

## Design facts a reviewer may want

- No telemetry and no accounts. Network requests happen when you start them, plus the daily update check if you turn it on: at most
  one successful check a day (a failed one is retried at most three times a day), two small downloads from github.com, the checksum
  list and its signature. GitHub sees your IP address and that the request comes from `curl`; no cookies, no identifier, nothing about
  your files (see [architecture](docs/architecture.md#stack)).
- Passwords and passphrases for `ssh` are asked in the window through a private Unix socket and are never written to disk, the
  environment or process arguments ([remote.md](docs/remote.md#authentication-prompts)).
- Until a folder is trusted: project settings cannot change `terminal.shell`, `lsp.servers` or schema associations, language
  servers do not start, schema downloads are limited to SchemaStore and `git status` runs with `core.fsmonitor` off
  (details: [architecture](docs/architecture.md#workspace-trust-in-one-place)).
- Extensions are in development and the current release loads none. The planned security model (WebAssembly in a sandboxed
  process, declared permissions enforced by the app, signed registry index, revocation) is in
  [plugin-architecture.md](docs/plugin-architecture.md#6-permissions-and-security). The registry's root is signed by two of
  three root keys, all held by the maintainer until a second maintainer takes one (see below).

## Extension registry keys

The public extension registry (docs/registry.md, "Running the public registry") is trusted through three Ed25519 root
keys compiled into the app; any two of them sign its `root.json`, which names the online key that signs the index
daily and expires after 90 days.

| Key | Held by | Kept |
|---|---|---|
| Root A | the maintainer | an encrypted USB stick |
| Root B | the maintainer (until a second maintainer takes it) | an encrypted USB stick in another place |
| Root C | the maintainer | a password manager, with two-step login |
| Online | the registry's server | `/srv/yard/keys/online.key`, readable by the service only; never backed up |

No root key is ever on the server. The public keys and the date of the ceremony are recorded in
`yard-ceremony-record.txt` with the release that ships them. A lost or stolen key is handled as in the registry's
runbook; report a suspected compromise privately as above.

## Updates and signatures

Every release has `SHA256SUMS.txt` (the SHA-256 of every other asset, including `RELEASE_NOTES.md`) and `SHA256SUMS.txt.sig`, an
Ed25519 signature over that file. The in-app updater accepts nothing that this signature does not cover. The public key is
`scripts/update-public-key.pem` (hex `47bc1afc0c2c70a4a72ffd0b5ba535397bd9ca83551dfc30a12d64d7cf96d67c`). To check a download, run
`scripts/verify-release.sh <tag>`; the steps are in [docs/releasing.md](docs/releasing.md#verify-a-download). If you suspect the
signing key is compromised, or find a release whose signature does not verify, report it privately as described above and do not
install it.

What the signature does not give you, and what is done about it:

- **No expiry, no revocation.** A signature stays valid for ever. Anyone who can answer a client's requests (a hostile network, a
  mirror, a proxy) can keep showing it an older, genuinely signed release (a freeze) or play an old list back (a rollback). The
  updater never offers a version that is not newer than the running one, and it remembers the highest version a verified list has
  offered on this installation and ignores lower ones afterwards. That only helps a client that has already seen the newer version:
  one that has never been offered 0.4.0 cannot tell a replayed 0.2.0 list from the real latest. The remedy for a key that leaked, and
  for old signed releases that should no longer be trusted, is a key rotation through a normal release
  ([docs/releasing.md](docs/releasing.md#signing-and-the-update-key)); clients that missed it have to reinstall by hand.
- **The installer is fetched from the tag of the verified version** (`releases/download/v<version>/`), not from `latest`, so a release
  published between the two requests cannot be mixed with the list. Its hash is checked before it is opened and again before it is
  installed.
- **Gatekeeper.** An update installed by the app does not go through macOS's first-launch check, because `curl` does not set the
  quarantine flag and Codeyard builds are not notarized yet. The protection is the signed checksum list, plus the check of the bundle
  identifier, the version and `codesign --verify` before the app is replaced.
- **Helper programs** (`curl`, `hdiutil`, `ditto`, `plutil`, `codesign`) are run from their system paths, never found through `PATH`
  on macOS (on Windows the `curl.exe` of the system folder).
