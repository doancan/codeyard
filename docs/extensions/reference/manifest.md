# Manifest reference

<!-- Generated from crates/ext-manifest/src/raw.rs: run `UPDATE_DOCS=1 cargo test -p ext-manifest reference` after changing it. -->

Every field of `codeyard-extension.toml` outside the contributions, which are in [Contributions](contributions.md). A field that is not listed here is an error (`CY0101`), so a typo is reported instead of ignored. Sizes and counts are in [Limits](limits.md), and what each error means is in [Errors](errors.md). The guide is the [developer guide](../developer-guide.md#the-manifest).

| Top level | Required | What it is |
| --- | --- | --- |
| [`[extension]`](#extension) | yes | Who the extension is: id, version, name, license. |
| [`[code]`](#code) | no | Where the extension's code is. Leave it out for themes, snippets and other no-code packs. |
| [`[permissions]`](#permissions) | no | What the code may reach: files, hosts, programs, accounts. Anything not listed is refused. |
| [`[activation]`](#activation) | no | Rarely needed: Codeyard starts the code when one of its contributions is first used. |
| `dependencies` | no | Other extensions this one needs, by full id, each with a minimum version: `"other.ext" = "1.2"`. |
| `untrusted` | no | `false` keeps the extension off in untrusted folders. By default it runs there, but cannot send, write or run anything. |

## extension

`[extension]`: Who the extension is: id, version, name, license.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `id` | string | yes | `<publisher>.<name>`, lowercase `a-z 0-9 -`. The publisher is claimed on first publish. |
| `version` | string | yes | `major.minor.patch`, with an optional pre-release suffix (`1.0.0-beta.1`). |
| `name` | string | yes | The display name. |
| `license` | string | yes | An SPDX expression, such as `MIT` or `MIT OR Apache-2.0`. |
| `description` | string | no | One or two sentences for the Extensions view. |
| `repository` | string | no | An `https://` URL of the source. |
| `categories` | array of strings | no | Words that group the extension in the Extensions view. |
| `icon` | string | no | A path to an image in the package. |

## code

`[code]`: Where the extension's code is. Leave it out for themes, snippets and other no-code packs.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `entry` | string | yes | `Cargo.toml` for Rust, a `.ts` or `.js` file for TypeScript or JavaScript. |

## permissions

`[permissions]`: What the code may reach: files, hosts, programs, accounts. Anything not listed is refused.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `read` | array of strings | no | Workspace globs the code may read. `.env*` and `.git/` must be named explicitly. |
| `write` | array of strings | no | Workspace globs the code may write. |
| `net` | array of strings | no | Hosts the code may reach over HTTPS: `api.example.com`, or `*.example.com` for its subdomains. |
| `net-from-setting` | string | no | A string setting (the key of a `[[settings]]` entry) that names a server of the user's, such as a company GitHub Enterprise: the user allows that host on the extension page, and the code may then reach it over HTTPS. |
| `auth` | array of strings | no | Accounts the code may sign requests with: `github`, or the id of an `[[oauth]]` table. It never sees the token. |
| `secrets` | boolean | no | Keychain entries of the extension's own. |
| `run` | array of strings | no | Programs the code may run, with any arguments, on the machine the workspace is on. |
| `clipboard` | string | no | `"read"`, `"write"` or `"read-write"`. |
| `terminal` | boolean | no | Send text to the integrated terminal. |
| `commands` | array of strings | no | `core:` commands the code runs (reference/core-commands.md). |
| `why` | string | no | One sentence for the install sheet. Required when the permissions are high risk. |

## activation

`[activation]`: Rarely needed: Codeyard starts the code when one of its contributions is first used.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `workspace-contains` | array of strings | no | Start the code in folders that contain a file matching one of these globs (files `.gitignore` ignores do not count). |
