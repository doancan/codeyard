# Captured crates.io data for the Cargo.toml dependency tests

These files are real answers from crates.io, saved once so the tests never use the network.

| File | What it is | Where it came from |
| --- | --- | --- |
| `serde.idx`, `tokio.idx`, `cc.idx` | Lines of a crate's sparse-index file (one JSON object per published version), a selection of the lines: first and last versions, a yanked one, a pre-release, a line with `features2` | `GET https://index.crates.io/se/rd/serde`, `…/to/ki/tokio`, `…/2/cc` |
| `crate-serde.json` | The answer for one crate, without its versions | `GET https://crates.io/api/v1/crates/serde?include=` |
| `version-serde-1.0.219.json` | The answer for one version (licence, size, …) | `GET https://crates.io/api/v1/crates/serde/1.0.219` |
| `search-serde.json` | The first three results of a search, without their `links` | `GET https://crates.io/api/v1/crates?q=serde&per_page=5` |

Captured on 2026-11-12 with `curl` and a `User-Agent` naming Codeyard. Nothing was changed except dropping lines and the
`links` objects named above, so the shape is what the servers send.

The data is about public crates and was published by crates.io; it is kept here as test input under crates.io's
[data access policy](https://crates.io/data-access) and [usage policy](https://crates.io/policies#crawlers) (identify the
client, at most one API request a second, prefer the sparse index). Each crate's own licence applies to the crate, not to
these metadata lines. The numbers in them (versions, download counts, timestamps) are a snapshot and will not match
crates.io today; the tests rely on the snapshot, never on the live values.
