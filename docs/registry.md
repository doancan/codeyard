# Registry

Yard is the extension registry: static files on a CDN plus a small publish service. This document describes what
exists today: the signed index format, how clients verify it and publisher signatures, the publish service, and the
operator's commands. Hosting and the runbook come with #70. The design is in [plugin-architecture.md §8](plugin-architecture.md#8-registry-and-publishing).

## Files

A registry is four files at one base URL (and the packages, by content digest):

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
          "code": false, "published": 1800000000, "status": "ok", "downloads": 31 }
      ]
    }
  }
}
```

- `digest` is the content digest the lock records; `sha256` is the `.cyx` file as downloaded; `permissions` is the
  sha256 of the permission set, so an update can tell "same permissions" without downloading.
- `status` is `"ok"`, `{"deprecated": {"replacement": "id"}}`, `"yanked"` or `{"revoked": {"reason": "…"}}`.
- A version with a suffix (`1.2.0-beta.1`) is a pre-release, offered only to users who opt in for that extension.
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
offline against the Fulcio CA and Rekor key that `root.json` pins:

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
GET  /index.json  /root.json …   the static files, for a registry without a CDN
```

An upload is the bundle's length (four bytes, big-endian), the bundle, then the package or statement. There are no
accounts: the identity in the bundle's certificate is the publisher.

A publish is accepted only when all of these hold:

- The package passes the same checks as `cyx check` and the app (`ext-pkg`), and contains no code (code extensions
  are accepted from Phase 2).
- The bundle is a valid publisher signature over this id, version and content digest (above).
- The publisher name (the part of the id before the dot) is unclaimed, or claimed by the same owner: a GitHub owner
  (by numeric id) or a browser-login account. The first publish claims it. A name with the same look as another
  owner's (`rnicrosoft`, `micros0ft`) or one letter away from it (`microsft`, for names of five letters or more) is
  refused.
- A new extension id or display name does not look like another owner's.
- An existing extension is published from the same repository and workflow as before.
- The version number has never been used for this id, even if that version was yanked.

Status statements are `codeyard-yard/v1/status
<id> <version> <yank|restore|deprecate [replacement]>
`, signed by the
identity that publishes the extension within the last 15 minutes. A revoked version stays revoked.

Every change writes the package first, then signs and writes a new index (signature, then index; a client that reads
between the two sees a mismatch and fetches again). `yard serve` also signs a fresh index at start and every 24 hours,
so it never expires while the service runs.

```text
yard init FOLDER [--registry NAME]           a new registry: keys, root.json (pinning public-good Sigstore), yard.json
yard serve CONFIG                            the service
yard resign CONFIG                           sign a fresh index once, for a scheduler instead of serve's timer
yard revoke CONFIG ID VERSION REASON…        the security role: stop a version on every client, with the reason
yard immediate CONFIG ID VERSION             the security role: let clients update to an advisory fix at once
```

`yard init` signs the root with one development key. For production the root key holders sign `root.json` with
`cyx yard sign root` (below). The pinned Sigstore roots are in `crates/yard/sigstore/`.

## Root key commands

```text
cyx yard keygen FILE                              a new key, readable only by you; prints the public key
cyx yard sign <root|index> FILE --key KEYFILE     add your signature to FILE.sig
cyx yard verify FOLDER --root-key HEX… [--threshold N]
```

A root key ceremony is one `keygen` per holder, then each holder runs `sign root root.json` on the same file; signing
again replaces that holder's signature. A test registry is the same with throwaway keys.
