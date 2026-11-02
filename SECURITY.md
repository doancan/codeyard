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
- schema, spell and parser code handling hostile input (crashes, unbounded memory or time);
- extension code escaping the extension host's sandbox (reaching files, the network or processes it should not), or
  reaching anything through the app that its granted permissions do not allow;
- an extension installed, updated or given more permissions without the consent sheet (or a permission prompt) the user
  should have seen;
- a forged, replayed or rolled-back registry index, or a package whose signature or digest should not have been accepted.

## Design facts a reviewer may want

- No telemetry and no accounts. Network requests happen when you start them, plus the daily update check if you turn it on: at most
  one successful check a day (a failed one is retried at most three times a day), two small downloads from github.com, the checksum
  list and its signature. GitHub sees your IP address and that the request comes from `curl`; no cookies, no identifier, nothing about
  your files (see [architecture](docs/architecture.md#stack)).
- Passwords and passphrases for `ssh` are asked in the window through a private Unix socket and are never written to disk, the
  environment or process arguments ([remote.md](docs/remote.md#authentication-prompts)).
- Extension accounts: tokens for an extension's GitHub or OAuth account, and its `secrets`, are kept in the system
  keychain under its publisher identity. Codeyard adds the token to the extension's requests to that account's hosts;
  the extension process never receives it. Each sign-in asks first, and the extension's page signs out
  ([plugin-architecture.md](docs/plugin-architecture.md#64-auth)).
- Until a folder is trusted: project settings cannot change `terminal.shell`, `lsp.servers` or schema associations, language
  servers do not start, schema downloads are limited to SchemaStore and `git status` runs with `core.fsmonitor` off
  (details: [architecture](docs/architecture.md#workspace-trust-in-one-place)).
- Extensions: code extensions are WebAssembly components run by `codeyard-exthost`, one process per active extension, never
  in the app. Before it loads any code the process locks itself down with the OS sandbox: on macOS a deny-default Seatbelt
  profile; on Linux landlock (required), a seccomp filter and, where user namespaces allow, an empty network namespace. It may only
  read its package and compiled code and write its own data folder; no network, no new processes. Everything else goes
  through the app, which checks each call against the permissions the user granted on the consent sheet (and Workspace
  Trust). On Windows the AppContainer sandbox has not passed its hardware check yet, so installed extensions with code do
  not run there; extensions without code do. Details:
  [plugin-architecture.md](docs/plugin-architecture.md#6-permissions-and-security). Registry packages are verified against
  a signed index; the registry's root is signed by two of three root keys, all held by the owner until a second maintainer
  takes one (see below).

## Extension registry keys

The public extension registry ([docs/registry.md](docs/registry.md#running-the-public-registry)) is trusted through
three Ed25519 root keys compiled into the app; any two of them sign its `root.json`, which names the online key that
signs the index daily and expires after 90 days. Until the root key ceremony has run, the app trusts no public
registry (`PUBLIC` in `crates/app/src/ext_registry.rs` is `None`), and no build compiles with placeholder keys there.

| Key | Held by | Kept |
|---|---|---|
| Root A | the owner (Doğancan) | an encrypted USB stick, at home: one of the two shares the owner signs with |
| Root B | the owner, until a second maintainer takes it | an encrypted USB stick in another place: the offline backup share |
| Root C | the owner | a Bitwarden secure note, with two-step login: the other share the owner signs with |
| Online | the registry's server | `/srv/yard/keys/online.key`, readable by the service only; never backed up (a restore rotates it) |
| Canary, deploy | GitHub Actions (environment `yard`, approved by the owner) | SSH keys that can each run one command on the server: revoke the canary extension, or install a `yard` binary that runs |
| Backups | the server and the owner's Bitwarden | an R2 token for the backup bucket only, in `/etc/yard/yard.env` (root only) |

No root key is ever on the server, and none is ever in this repository. The ceremony (`scripts/yard-root-ceremony.sh`)
writes `yard-ceremony-record.txt` with the date, the three public keys and where each private key is kept; the owner
signs it, and it is published with the release that first ships the keys, together with the keys in this section. A
lost or stolen key is handled as in the registry's runbook; report a suspected compromise privately as above.

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
- **Gatekeeper.** macOS releases are signed with Developer ID and notarized by Apple, but an update installed by the app does not go
  through macOS's first-launch check, because `curl` does not set the quarantine flag. The protection is the signed checksum list,
  plus the check of the bundle identifier, the version and `codesign --verify` before the app is replaced. Windows builds are not
  code-signed.
- **Helper programs** (`curl`, `hdiutil`, `ditto`, `plutil`, `codesign`) are run from their system paths, never found through `PATH`
  on macOS (on Windows the `curl.exe` of the system folder).
