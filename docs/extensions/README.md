# Codeyard extensions

Extensions add themes, snippets, keymaps, languages and language servers without code, and commands, views and
providers with code in TypeScript or Rust. You need only Codeyard for the first kind, and Node.js or Rust for the second.

## Start here

**[Quick start](developer-guide.md#quick-start)**: `cyx new`, `cyx dev`, `cyx check`, `cyx package`. A theme takes a few
minutes; [your first command](developer-guide.md#your-first-command) a few more.

## Guides

| Topic | Where |
| --- | --- |
| Themes, snippets, keymaps, languages, schemas, language servers | [A theme or snippet pack](developer-guide.md#a-theme-or-snippet-pack) |
| The manifest | [The manifest](developer-guide.md#the-manifest) |
| Commands, providers, files, network, programs (Rust) | [Commands](commands.md) |
| The same in TypeScript | [TypeScript](typescript.md) |
| Views | [Views](views.md) |
| Settings, status bar, quick pick, accounts, tree views and more | [Recipes](developer-guide.md#recipes) |
| Source control and pull request providers | [Source control](developer-guide.md#source-control), [Pull request providers](commands.md#pull-request-providers) |
| Permissions and what users see | [Permissions](developer-guide.md#permissions) |
| Testing | [Testing](testing.md) |
| Debugging with the Extension Inspector | [Test and debug](developer-guide.md#test-and-debug) |
| Publishing, pre-releases, the 48 h wait, yanking | [Publish](developer-guide.md#publish) |
| Coming from VS Code | [Coming from VS Code](developer-guide.md#coming-from-vs-code) |
| Moving from API 0.1 | [Migrating to API 1.0](migrating-to-1.0.md) |
| Something does not work | [Troubleshooting](developer-guide.md#troubleshooting) |

## Reference

Generated from the code, so they say what this version of Codeyard does. A test fails when one is out of date.

| Page | What it lists | Made from |
| --- | --- | --- |
| [Manifest](reference/manifest.md) | Every field of `codeyard-extension.toml` outside the contributions | `crates/ext-manifest/src/raw.rs` |
| [Contributions](reference/contributions.md) | Every contribution kind and its fields | `crates/ext-manifest/src/raw.rs` |
| [API](reference/api.md) | Every call, with its permission and the version that added it | `crates/ext-api/wit` |
| [Views](reference/ui.md) | Every view element and event | `crates/ext-ui` (checked, not generated) |
| [Core commands](reference/core-commands.md) | The Codeyard commands an extension may run | `crates/ext-manifest/src/core_commands.rs` |
| [cyx](reference/cli.md) | Every `cyx` command, option and environment variable | `crates/cyx/src/main.rs` |
| [Errors](reference/errors.md) | Every error code, what it means and its fix | `crates/ext-manifest`, `crates/ext-core` |
| [Limits](reference/limits.md) | Every size, count and time limit | `crates/ext-manifest/src/limits.rs` |

After changing what a page is made from, write it again:

```sh
UPDATE_DOCS=1 cargo test -p ext-manifest reference   # manifest, contributions, limits, core commands
UPDATE_DOCS=1 cargo test -p ext-api reference        # API
UPDATE_DOCS=1 cargo test -p cyx reference            # cyx, errors
```

## Examples

[examples/extensions](../../examples/extensions) has working extensions in both languages. CI builds them, runs their
unit tests and runs them in the extension host.

Every code snippet on these pages is checked in CI (`crates/cyx/src/doc_tests.rs`): Rust and TypeScript snippets must
be copied from an example that CI builds, manifests must pass the same checks as `cyx check`, and `cyx` commands must
exist with the options shown. A snippet that is not part of a full example lives in
[doc-snippets](../../examples/extensions/doc-snippets) or [doc-snippets-ts](../../examples/extensions/doc-snippets-ts).
