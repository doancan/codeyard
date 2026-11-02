# Registry

Yard is the extension registry: static files on a CDN plus a small publish service. This document describes the
signed index format, how clients verify it and publisher signatures, the publish service, the operator's commands, and
how the public registry is run (hosting, keys, monitoring and the runbook). The design is in [plugin-architecture.md §8](plugin-architecture.md#8-registry-and-publishing).

## Files

A registry is four files at one base URL (and, by content digest, the packages, their publisher signatures and their
details files):

| File | Signed by | Holds |
| --- | --- | --- |
| `root.json` | a threshold of the offline root keys (2 of 3 for the public registry) | registry name, the online key and its expiry (90 days), the pinned Fulcio CA and Rekor key |
| `root.json.sig` | | the root signatures |
| `index.json` | the online key named in `root.json` | every extension, its publisher identity and versions; expires 72 h after signing |
| `index.json.sig` | | the online signature |

The root keys are not in any file: the app ships them for the public registry, and `extensions.registries` names
another registry's keys. Each signature covers the file's exact bytes after a context line
(`codeyard-yard/v1/root.json` or `codeyard-yard/v1/index.json`), so there is no canonical JSON to agree on and a
signature for one file never passes for the other. A `.sig` file is `{"signatures": [{"key": hex, "sig": hex}]}`.

## Index

```json
{
  "format": 1, "registry": "yard.codeyard.dev", "version": 812, "expires": 1800259200,
  "extensions": {
    "you.night": {
      "name": "Night", "description": "A dark theme.", "categories": ["themes"],
      "publisher": { "issuer": "https://token.actions.githubusercontent.com", "owner_id": 7, "repository_id": 42,
                     "workflow": ".github/workflows/publish.yml" },
      "versions": [
        { "version": "1.0.0", "digest": "…", "sha256": "…", "size": 1200, "permissions": "…",
          "code": false, "published": 1800000000, "status": "ok", "downloads": 31, "details": "…" }
      ]
    }
  }
}
```

- `digest` is the content digest the lock records; `sha256` is the `.cyx` file as downloaded; `permissions` is the
  sha256 of the permission set, so an update can tell "same permissions" without downloading.
- `details` is the sha256 of `packages/<digest>.details.json`, the version's manifest and README for the Market
  (below).
- `downloads` counts every download of the version, updates included, per day and without logs.
- `status` is `"ok"`, `{"deprecated": {"replacement": "id"}}`, `"yanked"` or `{"revoked": {"reason": "…"}}`.
- A version with a suffix (`1.2.0-beta.1`) is a pre-release, offered only to users who opt in for that extension.
- A version with native programs has `targets`: one package per platform, `{ "<triple>": { "digest", "sha256",
  "size" } }` (`<id>-<version>-<triple>.cyx`). Its `digest`, `sha256` and `size` are those of the first platform
  published, and `code` is true. The client installs the workspace host's platform and records every platform's
  digest in the lock.
- Versions are listed oldest first and never reused. Times are Unix seconds.

## What a client checks

`ext_core::yard::verify` runs offline, in this order, and any failure accepts nothing:

1. Size limits (root 64 KiB, index 64 MiB, signatures 16 KiB).
2. `root.json` carries valid signatures from at least the threshold of **different** trusted root keys.
3. Its format is known, its version is not lower than the last one accepted, and the online key has not expired.
4. `index.json` is signed by that online key, names the same registry, and its version is not lower than the last
   one accepted.
5. Every entry is well formed: hashes are 64 hex digits, versions unique and in order, text bounded and free of
   control characters. Fields added in a later release of the same format are ignored, so the
   registry can grow without breaking older clients; the format number changes only for what they would misread.

An index past its expiry still verifies, marked stale: installed extensions keep running, revocations still apply,
and installs and updates wait for the user to confirm. The client keeps the last accepted root and index versions so
an older copy (from a cache or an attacker) is refused.

| Code | Meaning |
| --- | --- |
| CY0801 | a file is larger than its limit |
| CY0802 | a file is not valid JSON or has a bad field |
| CY0803 | written in a newer format: update Codeyard |
| CY0804 | not signed by enough trusted keys |
| CY0805 | the online key has expired |
| CY0806 | older than a copy already accepted |
| CY0807 | the index belongs to another registry |
| CY0810 | the Fulcio CA or Rekor key pinned in `root.json` is unusable |
| CY0811 | the publisher signature is not a bundle this version reads |
| CY0812 | the certificate does not chain to the pinned CA at the logged time, or names no publisher |
| CY0813 | the transparency log evidence is missing or does not hold |
| CY0814 | the signature is not over this package |

## Publisher signatures

The index proves the registry listed a version; the publisher signature proves the publisher built it, so a
compromised registry cannot slip in a package of its own. Publishers sign keyless with Sigstore: in GitHub Actions the
workflow's OIDC token gets a 10-minute Fulcio certificate, and the signing is logged in Rekor. Nobody holds a key.

What is signed is a statement, not the package, so a signature can never be moved to another extension or version:

```text
codeyard-yard/v1/package
you.night 1.2.0 <content digest>
```

`ext_core::publisher::verify_package` checks a Sigstore bundle (v0.1 to v0.3, a message signature logged in Rekor v1)
offline against the Fulcio CA and Rekor key that `root.json` pins. The bundle must have the shape of the version it
names (v0.3 one signing certificate, v0.1 and v0.2 a chain of one), so a bundle cannot be relabelled; then:

1. Rekor's signed entry timestamp over the entry and the time it was logged.
2. The inclusion proof up to its root hash and Rekor's signed checkpoint for that root (required from bundle v0.2).
3. The certificate chains to the pinned Fulcio CA, for code signing, at the logged time.
4. The logged entry names this certificate, this signature and the statement's hash; so does the bundle's message digest.
5. The signature is the certificate key's over the statement.

The publisher identity comes from the certificate: for GitHub Actions the issuer, the numeric owner and repository ids
(they survive renames) and the workflow file without its ref; for a browser login the issuer and the account. The first
registry install of an id pins that identity. Another identity later is a new trust decision, shown with the full
consent sheet. A sideloaded package with the same id starts with no pin.

Rekor v2 entries carry no time of their own (a timestamp authority provides it). The public-good Sigstore keeps v1 as its
default log and announces a freeze a year ahead, so v2 support is added when that happens. SCTs are not checked: the CA is
pinned, so they would add a second log, not a second trust decision. The tests run the relevant cases of the Sigstore
conformance suite (`crates/ext-core/testdata/sigstore`).

## The publish service

`crates/yard` builds the `yard` binary, which runs on the registry's server. Everything users download is static and
lives in its public folder, which a CDN serves: `root.json`, `index.json`, their signatures, and
`packages/<digest>.cyx`. State is one SQLite file.

```text
POST /v1/publish                 a Sigstore bundle and a package          → what was published, or why not
POST /v1/status                  a Sigstore bundle and a status statement → yank, restore, deprecate
GET  /packages/<digest>.cyx      the package; counts one download for the day (nothing about who asked is kept)
GET  /packages/<digest>.sigstore.json   its publisher signature; clients verify it themselves
GET  /packages/<digest>.details.json     its manifest and README, for the Market (not counted as a download)
GET  /index.json  /root.json …   the static files, for a registry without a CDN
GET  /healthz  /metrics          "ok"; index version and expiry, online key expiry, counts (Prometheus text). The
                                 proxy answers 404 for /metrics, so only the server itself reads it
```

An upload is the bundle's length (four bytes, big-endian), the bundle, then the package or statement. There are no
accounts: the identity in the bundle's certificate is the publisher.

A publish is accepted only when all of these hold:

- The package passes the same checks as `cyx check` and the app (`ext-pkg`), and contains no code and no native
  program, unless its publisher name is invited (below).
- The bundle is a valid publisher signature over this id, version and content digest (above).
- The publisher name (the part of the id before the dot) is unclaimed, or claimed by the same owner: a GitHub owner
  (by numeric id) or a browser-login account. The first publish claims it. A name with the same look as another
  owner's (`rnicrosoft`, `micros0ft`) or one letter away from it (`microsft`, for names of five letters or more) is
  refused.
- A new extension id or display name does not look like another owner's.
- An existing extension is published from the same repository and workflow as before.
- The version number has never been used for this id, even if that version was yanked.

### Native programs

Code and native programs are accepted by invitation only. `yard.json` lists the invited publisher names:

```json
{ "invited": ["dogan"] }
```

Restart `yard serve` after a change. Without the list (the default) the registry takes data-only extensions, as in
Phase 1. An invited publisher's packages may run code (Wasm), and may carry native programs:

- One package per platform (`TARGET` names it), up to 128 MiB, uploaded one by one. A platform joins its version when
  the version has native programs, the platform is new, and the manifest and README are the same as the others'. A
  platform once published is never replaced.
- **Review.** A publisher's first native program waits for a human. The upload is recorded and answered with
  `"review": true`, but the version is left out of the index, and its files wait in `review/` beside the database, out
  of the public folder. More platforms of that version wait with it. The reviewer works from the server's command line:

```text
yard review list CONFIG                       what waits: id, version, publisher, the install sheet's lines and each
                                              platform's package, to inspect
yard review approve CONFIG ID VERSION         move its files into the public folder and sign an index that lists it;
                                              the publisher's later native programs are listed without review
yard review reject CONFIG ID VERSION REASON…  never list it and delete its files; its version number stays used
```

Native programs are code: clients update to them after 48 h like any code update.

Status statements are `codeyard-yard/v1/status
<id> <version> <yank|restore|deprecate [replacement]>
`, signed by the
identity that publishes the extension within the last 15 minutes. A revoked version stays revoked.

With each package the service writes its details file: `{"manifest": "<codeyard-extension.toml>", "readme":
"<README.md>"}`, the README cut at a line to 256 KiB (`"readme_cut": true` then), so the Market shows a page without
downloading the package. A registry from before details files gets them for every version when `yard` next starts.

Every change writes the package first, then signs and writes a new index (signature, then index; a client that reads
between the two sees a mismatch and fetches again). `yard serve` also signs a fresh index at start and every 24 hours,
so it never expires while the service runs.

```text
yard init FOLDER [--registry NAME] [--root-keys HEX,… --threshold N] [--fulcio PEM --rekor PEM]
                                             a new registry: online key, root.json (pinning public-good Sigstore,
                                             or the given roots for a rehearsal), yard.json; with the production
                                             root keys, root.json waits for them
yard serve CONFIG                            the service
yard rotate CONFIG                           the next online key and root.next.json for the root key holders to sign
yard promote CONFIG                          check their signatures, put the new root and key in place, sign
yard resign CONFIG [--expired]               sign a fresh index once (yard-resign.timer runs it daily); --expired
                                             signs one already past its expiry, to see clients refuse it (staging)
yard status CONFIG [--hours N] [--key-days N] index and online key expiry, counts; fails when the index expires
                                             within N hours (24) or the key within N days (14): the hourly alert
yard revoke CONFIG ID VERSION REASON…        the security role: stop a version on every client, with the reason
yard immediate CONFIG ID VERSION             the security role: let clients update to an advisory fix at once
yard review list | approve | reject …        first native programs waiting for a reviewer (above)
```

Without `--root-keys`, `yard init` signs the root with one development key made on the spot. With them no root key is
ever on the server: the holders sign `root.json` with `cyx yard sign root` (below), and `yard.json` records the keys
and threshold so `yard promote` can refuse a root they did not sign. `yard serve` warns daily once the online key has
less than 14 days left. The pinned Sigstore roots are in `crates/yard/sigstore/`.

## The client

`ext_core::registry` is the client's side, without a network of its own; the app hands it `curl` (as the updater does,
so proxies and the system's certificates apply). Each registry's last accepted copy and its state (versions seen,
when fetched, the index's ETag) are kept in `<data>/extensions/registries/<name>/`.

- **When it asks.** Only for a registry an installed extension came from, at most once a day (a conditional request:
  a 304 reads nothing), when the user opens the Market (the copy from earlier the same day is used), or when the user
  asks for an install. Searching the Market asks nothing. Opening an extension's page fetches its details file once;
  it is kept in `details/<sha256>.json` and checked against the index's `details` digest, its manifest against the
  version's id, version and permission digest. A user with no extension from a registry who never opens the Market
  makes no request; a test fails on any.
- **The Market** searches the indexes on the device: the name, id, description and categories, every word of the
  query somewhere, names before ids, categories and descriptions, ties most downloaded first. It runs off the UI
  thread on each keystroke; 10 000 extensions take well under a millisecond, and the perf row
  `market.search_ms` (key press to the results in the view, ceiling 16 ms) keeps it there. A README is shown
  without images (none is requested) and its links open in the browser only after the user confirms the address.
- **What it checks before the consent sheet**, in order: the index (above); the version may be installed (not yanked
  or revoked); the package's size and sha256 against the index, its content digest, id, version and permission
  digest; the publisher's bundle (`packages/<digest>.sigstore.json`) against the root's pinned Sigstore roots; and
  that the signer is the publisher the index lists. Any failure installs nothing.
- **Pinning.** The lock records `{ "kind": "registry", "registry": name, "publisher": … }`. A later install signed by
  another identity shows the full sheet with that called out; `--install-extension` refuses it. A lock with a
  registry entry is format 2, one with a pre-release opt-in (`"pre-release": true`) format 3, one with per-platform
  digests (`"targets": { "<triple>": "<digest>" }`, native programs) format 4; a lock is written in the lowest format
  that holds it, so an older Codeyard still reads it.
- **Native programs.** The client downloads the package for the workspace host's platform (this machine's, or the
  SSH server's, from the agent's report) and checks it like any other, plus its `TARGET`. Another platform's package
  is fetched when a workspace on that platform first runs the program, checked the same way and against the digest
  the lock recorded for it. The program's bytes are copied to the workspace host with the agent install's steps (a
  temporary file, `sha256sum`, then `mv`); the host downloads nothing, and each run first checks the copy's hash.
- **Stale index.** Past its 72 h, installs and updates wait for the user to confirm on the sheet;
  `--install-extension` refuses. Installed extensions keep running.
- **Command line consent.** `--install-extension` prints the sheet of a version that asks for permissions and installs
  it only with `--accept-permissions`, from a registry as from a file.
- **Checking now.** `codeyard --update-extensions` runs the update check below without a window, asking each registry
  an installed extension came from at once (the person asked, so not only once a day), and prints what changed.
- **Updates and version status** (`ext_core::update` decides, `app/src/ext_updates.rs` acts). Each time the index is
  read for an installed extension (when settings load, and hourly in a long session; the registry is still asked at
  most once a day), every extension from that registry is checked:
  - The installed version's `status`: `revoked` is written to `<data>/extensions/revoked.json` by content digest
    (also for the previous version a rollback would go to), and `ext::reload` stops it and, transitively, every
    extension that depends on it, offline and after a restart too, until a newer version replaces it. The file is
    watched, so every window follows. `yanked` goes back to the newest earlier installable version at once (from the
    store when it is the previous version, else downloaded with every check), unless a newer version may install now.
    `deprecated` is shown with its replacement.
  - A newer version (`latest`, with pre-releases when the lock opts in): `extensions.autoUpdate: install` installs it
    when it is ready, data-only at once and code at `published + 48 h` (at once with `immediate`, or for a
    pre-release on the channel). The permission digest says "the same" with no download; otherwise the version's
    details file gives its manifest, and `Permissions::added_since` what it asks beyond the installed version. Nothing
    new installs silently; more keeps the installed version and offers a card with only the added lines; a publisher
    other than the pinned one, or a stale index, offers the full sheet. The downloaded package's own manifest decides
    again before anything is written. `notify` and `off` download nothing (not even the details file) and show the
    version. **Update Now** is the same without the wait; the card's **Update** accepts exactly the lines it showed.
  - One check runs at a time per process, so two windows do not download the same update.
- **Registries.** The public registry (its root keys arrive with the key ceremony, #70) and `extensions.registries`
  in the user's settings (never a project's): `{ "name", "url", "root-keys", "threshold" }`.

## Root key commands

```text
cyx yard keygen FILE                              a new key, readable only by you; prints the public key
cyx yard sign <root|index> FILE --key KEYFILE     add your signature to FILE.sig; --key - reads the key from
                                                  standard input (pasted from a password manager, never on disk)
cyx yard verify FOLDER --root-key HEX… [--threshold N]
cyx yard check URL --root-key HEX… [--threshold N] [--warn-days N] [--revoked ID@VERSION]
                                                  a live registry as clients see it: fails when the index expires
                                                  within a day, the online key within N days (14), or ID@VERSION
                                                  is not revoked yet
cyx yard fixture FOLDER --extensions N --url URL  a signed index of N made-up extensions, for the Market's perf run
```

A root key ceremony is one `keygen` per key, then the holders run `sign root root.json` on the same file; signing
again replaces that key's signature. `scripts/yard-root-ceremony.sh` does it for the public registry (below). A test
registry is the same with throwaway keys.


## Running the public registry

The owner's part (accounts, the server, the key ceremony) is in [registry-setup-checklist.md](registry-setup-checklist.md),
in the order it happens. This section is how the registry runs once it is live.

### Hosting

| Part | Choice | Cost |
|---|---|---|
| Server | One Hetzner Cloud CX22 (2 vCPU, 4 GB, 40 GB disk, 20 TB traffic) or its successor, Ubuntu 24.04 LTS, Nuremberg or Falkenstein | about 5 € a month with its IPv4 address (+1 € for Hetzner's daily disk backups, optional) |
| Address, CDN, TLS | Cloudflare (free plan): DNS, the proxy, the public certificate; the server holds a Cloudflare Origin CA certificate (15 years) and Cloudflare connects to it with SSL mode **Full (strict)** | free |
| Backups | Cloudflare R2: Litestream copies the database as it changes; `yard-backup` copies the files and a database snapshot daily | free up to 10 GB, then 0.015 $ per GB a month; no fee for reading |
| Alerts | `yard-alert`: a webhook (Slack, Discord, ntfy) or e-mail over SMTP, set in `/etc/yard/yard.env` | free |
| Monitoring from outside, canary, deploy | GitHub Actions in this repository; a failed run e-mails the owner | free |
| Domain | `yard.<domain>` | about 12 $ a year |

About 6 € a month and the domain. Why: `yard` is one binary with one SQLite file and one folder, so a small server
holds it for a long time (an index of 10 000 extensions is a few MB; a 304 answers most daily requests), and nothing
needs a database server, containers or a cloud account beyond these. #70 first named Postgres; SQLite with Litestream
replaced it when the service was written (#65): one writer, a few writes a day, and a backup that is a file copy.
Security updates install by themselves (`unattended-upgrades`, reboot at 04:00).

**What runs on the server** (`deploy/yard/setup.sh` installs all of it):

| Unit | When | Does |
|---|---|---|
| `yard.service` | always | `yard serve` on 127.0.0.1:8080; signs the index at start and every 24 h |
| `caddy.service` | always | TLS on port 443 with the origin certificate, in front of yard; `/metrics` is not passed on |
| `litestream.service` | always | copies `yard.sqlite` to R2 as it changes |
| `yard-resign.timer` | daily, 03:17 UTC | `yard resign`: a second daily signature, so the index (72 h) never expires while either runs |
| `yard-watch.timer` | hourly | `yard-watch`: `/healthz` answers, `yard status` (the index does not expire within 24 h, the online key not within 14 days), Litestream and Caddy run |
| `yard-backup.timer` | daily | `yard-backup`: the files (never the online key) and a database snapshot (kept 35 days) to R2 |
| `yard-restore-check.timer` | weekly | `yard-restore --check`: restores the backup into a temporary folder and checks it |
| `yard-firewall.timer` | monthly | `yard-firewall`: HTTPS from Cloudflare's current address list only; SSH from anywhere, rate-limited, by key |
| `yard-alert@.service` | when one of the above fails | sends the unit's name and last 40 log lines to the alert channel; `yard-watch` repeats it hourly until fixed |

Logs go to the journal (`journalctl -u yard`), which keeps at most 1 GB and one month
(`/etc/systemd/journald.conf.d/yard.conf`). The secrets are in one file, `/etc/yard/yard.env` (root only; the template
is `deploy/yard/yard.env.example`), and the origin certificate in `/etc/caddy/origin.{pem,key}`.

**Uploads through Cloudflare.** The free plan takes request bodies up to 100 MB, so a per-platform package over about
100 MB cannot be published through the proxy (the registry's own limit is 128 MiB). Nothing else is affected; if a
publisher needs it, the choice is Cloudflare's Business plan or a second, unproxied address for `POST /v1/publish`.

**Moving.** Everything is `/srv/yard` (and the online key in `/srv/yard/keys`): on the new server run `setup.sh`, copy
`/etc/yard/yard.env`, stop `yard` on the old one, copy `/srv/yard` over (`rsync -a`, or `yard-restore` from R2 and a
new online key as in Restore below), start it and change the DNS record in Cloudflare. Clients only know the URL and
the root keys, so nothing changes for them. Moving the URL itself needs an app release with the new address. Another
provider works the same way: any Linux VPS with systemd; another CDN needs an origin certificate of its own and its
address list in `yard-firewall`.

**Privacy.** `yard` logs nothing about who asks, and Caddy's access log is off. Cloudflare's proxy sees each request's
IP address like any CDN; its logs are not enabled on this account. Every client fetches the same index, so nobody
learns what anyone installed.

**Caching.** `index.json`, `root.json` and their signatures are `no-cache` with an ETag: every request reaches the
service (a 304 when nothing changed), so the index and its signature always match and a revocation is visible at once.
Packages are `private`: no shared cache keeps them, so every download is counted; signatures and details files are
immutable and may be cached anywhere.

### Root key ceremony

Three Ed25519 root keys, any two of which sign `root.json`. The owner holds all three until a second maintainer takes
one: **A** on an encrypted USB stick at home, **B** on another one kept somewhere else, **C** in the owner's Bitwarden
vault (a secure note, with two-step login on). Losing one key loses nothing; one stolen key cannot sign anything.
SECURITY.md lists who holds what.

1. Erase two USB sticks in Disk Utility as "APFS (Encrypted)", each with its own password, and insert both.
2. `CYX=target/release/cyx scripts/yard-root-ceremony.sh /Volumes/YARD-A /Volumes/YARD-B`, with the network off.
   It shows key C once for the Bitwarden note, checks that the note holds it by reading it back from the clipboard,
   writes A and B to the sticks and `yard-ceremony-record.txt` (public keys only).
3. The record has every line to copy: the `yard init` command for the server, the `PUBLIC` constant for the app and
   the `YARD_ROOT_KEYS` variable for GitHub. The owner fills in its checks and signs it; it is kept with the release
   that ships the keys, and its public keys go into SECURITY.md.

The app gets the keys in `crates/app/src/ext_registry.rs`:

```rust
pub const PUBLIC: Option<(&str, &str, &[&str], usize)> =
    Some(("yard.<domain>", "https://yard.<domain>", &["<A>", "<B>", "<C>"], 2));
```

and `cyx publish` its default address in `crates/cyx/src/publish.rs` (`DEFAULT_REGISTRY`). The build refuses a
`PUBLIC` that could not be real: a key that is not 64 lowercase hex digits or has fewer than 8 different digits (a
placeholder such as `000…`), the same key twice, a threshold below 2 or above the number of keys, or an address that
is not https or names `example` (`ext_registry::public_problem`, checked when the crate compiles, in every profile).

Signing later (`root.json` once, `root.next.json` every 90 days), with any two:

```text
cyx yard sign root root.next.json --key /Volumes/YARD-A/yard-root-a.key
pbpaste | cyx yard sign root root.next.json --key -        key C, copied from the Bitwarden note
```

`scripts/yard-root-ceremony.sh --rehearsal DIR` makes three throwaway keys in `DIR` without questions, for the
rehearsal below.

### Deploy

Run the `yard-deploy` workflow. It builds `yard` in release mode on Ubuntu 24.04, keeps it as an artifact, and sends
it to the server, where `yard-install-binary` installs it only if it runs (the previous one stays as
`/usr/local/bin/yard.previous`) and restarts the service. The deploy key can do nothing else.

### Monitoring

Two checks, so either catches what the other cannot see:

- **On the server**, hourly: `yard-watch` (above). It alerts through `yard-alert` when the index expires within 24 h
  (neither `yard serve` nor `yard-resign` signed), the online key within 14 days, or a service is down.
- **From outside**, hourly: the `yard-monitor` workflow runs `cyx yard check` against the public URL with the root
  keys, through Cloudflare, as clients see it. A failed run e-mails the owner.

`curl -s 127.0.0.1:8080/metrics` on the server prints the index version and expiry, the online key's expiry, the
number of extensions, versions, revoked versions and reviews waiting (Prometheus text, if a scraper is added later).

### The canary

`yard-canary` runs weekly: it publishes `codeyard.canary` 0.0.<run> (`deploy/canary`, a snippet nobody should
install), revokes it on the server through the canary key (which can revoke that extension and nothing else), and
polls the signed index with `cyx yard check --revoked` (the client's own verification) until the revocation is there.
It fails after 2 hours. Each run adds a line to `canary.tsv` on the `yard-canary-log` branch: run, version, when it was
published, revoked and seen in the index, the minutes, pass or fail. Clients fetch the index at most once a day, so an
online client stops a revoked version within 24 h of the time seen. For the release notes:

```text
git fetch origin yard-canary-log && git show origin/yard-canary-log:canary.tsv | tail -n 3
```

### Rehearsal

`deploy/rehearse.sh [FOLDER]` runs the whole registry on one machine without a server, accounts or the network, and
CI runs it on Linux: a stand-in Sigstore (`crates/yard/examples/sigstore_stand_in.rs`, built from the test authority,
never deployed), a ceremony with throwaway keys, `yard init` with them (one signature is refused, two accepted),
a data-only extension published, code refused from a publisher who is not invited and published by one who is, both
installed with `codeyard --install-extension` (the code extension prints its sheet and is refused until
`--accept-permissions` is added), the code extension revoked (the index shows it, `codeyard
--update-extensions` stops it on the client that has it, another client refuses to install it with the reason), and an
index signed already expired (`yard resign --expired`): `yard status` and `cyx yard check` fail, and a client refuses
to install from it and says the list is more than 3 days old. Each client keeps its files in the rehearsal's folder.
On a Mac without Xcode's Metal toolchain: `REHEARSE_APP_FEATURES=gpui/runtime_shaders deploy/rehearse.sh`.

### Runbook

Commands marked `server$` run on the registry's server, `you$` on the owner's computer.

**Rotate the online key** (every 90 days; both monitors warn 14 days ahead):

```text
server$  sudo -u yard yard rotate /srv/yard/yard.json
you$     scp server:/srv/yard/root.next.json .        and sign it with two keys (above)
you$     scp root.next.json.sig server:/tmp/  then on the server: sudo -u yard cp /tmp/root.next.json.sig /srv/yard/
server$  sudo -u yard yard promote /srv/yard/yard.json && sudo systemctl restart yard
```

`promote` refuses a root without two valid signatures; until it runs, the old key keeps signing.

**Revoke a version** (malware, stolen publisher identity): `sudo -u yard yard revoke /srv/yard/yard.json ID VERSION
REASON…`. The reason is shown to users. Clients stop it on their next index fetch (at most a day), with every
extension that depends on it; `codeyard --update-extensions` checks at once. A revoked version stays revoked. If the
publisher's identity was stolen, also take the publisher out of `invited` (below) until it is sorted out.

**Shorten the wait for an advisory fix**: `sudo -u yard yard immediate /srv/yard/yard.json ID VERSION`. Clients update
to it without the 48 h wait for code, on their next index fetch.

**Invite a publisher to publish code**: add the publisher name (the part of the id before the dot) to `invited` in
`/srv/yard/yard.json`, then `sudo systemctl restart yard`:

```text
server$  sudo -u yard python3 -c 'import json,sys; p="/srv/yard/yard.json"; c=json.load(open(p)); c.setdefault("invited",[]).append(sys.argv[1]); json.dump(c,open(p,"w"),indent=2)' NAME
server$  sudo systemctl restart yard
```

The name must already belong to that publisher (their first publish of a data-only extension claims it), or they
claim it with their first code publish. Taking a name out stops new code from them; what is published stays.

**Review a first native program**: the publisher's upload is answered with `"review": true` and waits unlisted.

```text
server$  sudo -u yard yard review list /srv/yard/yard.json      id, version, publisher, the install sheet's lines, each platform's package
server$  mkdir /tmp/r && tar -xzf /srv/yard/review/<digest>.cyx -C /tmp/r       unpack one (the path list prints) to read it
server$  sudo -u yard yard review approve /srv/yard/yard.json ID VERSION
     or  sudo -u yard yard review reject /srv/yard/yard.json ID VERSION REASON…
```

Approve when the program comes from the repository and workflow the publisher signature names (`review list`
shows the publisher; the certificate is in `review/<digest>.sigstore.json`), does what the README and the permissions
say, and nothing else. Approval lists it at once
and lets the publisher's later native programs through without review; code still waits 48 h on clients.

**Rotate the canary or deploy key** (a leaked GitHub secret, or yearly): make a new key, put its public half on the
server and the private half in the GitHub environment `yard`; `yard-authorize` replaces the old line:

```text
you$     ssh-keygen -t ed25519 -N '' -C yard-canary -f yard-canary
server$  sudo yard-authorize canary "$(cat yard-canary.pub)"
you$     gh secret set YARD_CANARY_SSH_KEY --env yard < yard-canary && rm yard-canary yard-canary.pub
```

The same with `deploy` and `YARD_DEPLOY_SSH_KEY`. The canary's publisher identity is this repository's
`yard-canary` workflow; renaming that file needs a new canary id (`deploy/canary`), since the old one stays bound to it.

**Restore from backup** (a lost or broken server): create a server and run `setup.sh` as in the checklist, put the old
`/etc/yard/yard.env` back (its copy is in the owner's Bitwarden), then:

```text
server$  sudo yard-restore                                   files from R2, the database from Litestream (or the newest snapshot)
server$  sudo -u yard yard rotate /srv/yard/yard.json        the online key is never backed up
you$     sign root.next.json with two root keys and copy the signatures back (as in Rotate)
server$  sudo -u yard yard promote /srv/yard/yard.json
server$  sudo systemctl start yard litestream
```

Point the DNS record at the new server. Clients accept the new online key because the new root is signed by the root
keys they ship with. `yard-restore --check` (weekly) proves the same steps work without touching `/srv/yard`.

**An alert arrives**: its first line names the unit. `sudo systemctl status UNIT` and `journalctl -u UNIT -n 100` say
why; `sudo -u yard yard status /srv/yard/yard.json` shows the index and key. An index about to expire is fixed by
`sudo systemctl restart yard` (it signs at start) or `sudo -u yard yard resign /srv/yard/yard.json`.

**A root key is lost**: the other two still sign. Make a replacement with `cyx yard keygen` and ship the new set of
public keys in an app release; until enough clients have it, keep signing with two of the old keys.

**A root key is stolen**: one key cannot sign anything, but replace it as above, soon. If two are stolen, publish a
security advisory and ship new root keys in an app release at once.
