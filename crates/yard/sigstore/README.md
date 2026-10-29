The public-good Sigstore roots Yard pins in `root.json` by default (`yard init`): the current Fulcio CA chain
(intermediate, then root) and the Rekor v1 log key (`rekor.sigstore.dev`), taken from
https://github.com/sigstore/root-signing `targets/trusted_root.json`. When Sigstore rotates them, update these files and
sign a new `root.json` (docs/registry.md).
