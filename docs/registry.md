# Registry

Yard is the extension registry: static files on a CDN plus a small publish service. This document describes what
exists today: the signed index format, how clients verify it, and the operator's commands. Hosting and the runbook
come with #70. The design is in [plugin-architecture.md §8](plugin-architecture.md#8-registry-and-publishing).

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
                     "workflow": "you/night/.github/workflows/publish.yml" },
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

## Operator commands

```text
cyx yard keygen FILE                              a new key, readable only by you; prints the public key
cyx yard sign <root|index> FILE --key KEYFILE     add your signature to FILE.sig
cyx yard verify FOLDER --root-key HEX… [--threshold N]
```

A root key ceremony is one `keygen` per holder, then each holder runs `sign root root.json` on the same file; signing
again replaces that holder's signature. A test registry is the same with throwaway keys.
