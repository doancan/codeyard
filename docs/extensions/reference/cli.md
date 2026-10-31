# cyx

<!-- Generated from crates/cyx/src/main.rs and yard.rs: run `UPDATE_DOCS=1 cargo test -p cyx reference` after changing them. -->

`cyx` is the command-line tool for extension authors. It ships with Codeyard: **Extensions: Install cyx Command in PATH** in the palette adds it. Everything it checks is checked by the same code the app and the registry run, so a folder `cyx check` accepts installs. It exits with 0 on success and 1 on any error, and prints errors in the [format of the error reference](errors.md).

## cyx --help

```text
cyx: build Codeyard extensions

usage:
  cyx new <kind> <name> [--publisher NAME]   start an extension (kind: theme, snippets, keymap, language,
                                             language-server, command, view, pull-requests; the code ones are Rust,
                                             or TypeScript with --lang ts; --sdk PATH takes the SDK from a Codeyard
                                             checkout)
  cyx dev [FOLDER] [-- CODEYARD ARGS…]       run Codeyard with it loaded; it reloads when you save
  cyx check [FOLDER] [--fix]                 list every problem with its file, line and fix; --fix adds the
                                             missing permissions to codeyard-extension.toml
  cyx explain CODE                           what an error code such as CY0201 means and how to fix it
  cyx package [FOLDER] [-o FILE|FOLDER]      build the code, then write a .cyx to install or publish
                                             (--no-build: package the extension.wasm already there;
                                             --target TRIPLE: one platform's package of an extension with
                                             native programs in native/<TRIPLE>/; without it, every platform)
  cyx test [FOLDER] [--fixtures FILE]        build the code and run each command and view in the extension host;
                                             FILE (default test/fixtures.json) answers its HTTP requests
  cyx import-vscode <FOLDER | FILE.vsix> [-o FOLDER]
                                             convert a VS Code theme, snippets, keymap or language
  cyx publish [FOLDER] [--dry-run] [--pre-release] [--registry URL] [--target TRIPLE]
                                             sign it (no keys: GitHub Actions or a browser sign-in) and publish
                                             (native programs: every platform in native/, or the one named)
  cyx yank | restore VERSION [FOLDER]        hide a published version from installs, or bring it back
  cyx deprecate VERSION [FOLDER] [--replacement ID]
  cyx yard <keygen|sign|verify> …           the registry operator's commands (cyx yard help)
  cyx --version

FOLDER defaults to the current folder. Docs: docs/extensions/README.md
```

## cyx yard help

The registry operator's commands. Authors do not need them.

```text
registry operators:
  cyx yard keygen FILE                       write a new signing key; prints its public key
  cyx yard sign <root|index> FILE --key KEY  add a signature to FILE.sig (--key - reads the key
                                             from standard input, e.g. pasted from a password manager)
  cyx yard verify FOLDER --root-key HEX… [--threshold N]
                                             check root.json, index.json and their signatures
  cyx yard check URL --root-key HEX… [--threshold N] [--warn-days N] [--revoked ID@VERSION]
                                             check a live registry as clients do; fails when the index
                                             expires within a day or the online key within N days
                                             (default 14), or ID@VERSION is not revoked yet
  cyx yard fixture FOLDER --extensions N --url URL
                                             a signed index of N made-up extensions (no packages) for
                                             trying the Market and its performance run; prints the
                                             extensions.registries entry that trusts it
```

## Environment

| Variable | What it changes |
| --- | --- |
| `CODEYARD_REGISTRY` | The registry `cyx publish`, `yank`, `restore` and `deprecate` use when there is no `--registry`. |
| `CODEYARD_BIN` | The Codeyard `cyx dev` starts, instead of `codeyard` on `PATH` or the installed app. |
| `CODEYARD_EXTHOST` | The `codeyard-exthost` `cyx test` runs, instead of the one beside `cyx` or on `PATH`. |
| `CODEYARD_CYX_CACHE` | Where cyx keeps the TypeScript build tools it installs, instead of the platform's cache folder. |
| `SIGSTORE_ID_TOKEN` | An OIDC token for signing, instead of GitHub Actions' token or a browser sign-in. |
| `CODEYARD_FULCIO_URL` | Another Sigstore certificate authority (a private Sigstore, or tests). |
| `CODEYARD_REKOR_URL` | Another Sigstore transparency log. |
| `CODEYARD_OIDC_URL` | Another OIDC issuer for the browser sign-in. |
| `CARGO` | The cargo that builds Rust extensions. |
| `RUSTC` | The rustc cyx asks for its targets. |
