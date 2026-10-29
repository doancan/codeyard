# Sigstore fixtures

Real bundles from the Sigstore conformance suite (https://github.com/sigstore/sigstore-conformance,
`test/assets/bundle-verify`, Apache-2.0), kept as they are: each folder has a `README` saying what it tests, and a
name ending in `_fail` must be refused. `a.txt` is the artifact they sign unless the folder has its own `artifact`.
`trusted_root.json` is the public-good production root from https://github.com/sigstore/root-signing
(`targets/trusted_root.json`, Apache-2.0); a folder with its own `trusted_root.json` uses that one.

Only the cases for message signatures logged in Rekor v1 are here, which is what `ext_core::publisher` verifies.
