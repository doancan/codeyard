# Errors

<!-- Generated from ext-manifest's `Code` and ext-core's registry and publisher codes: run `UPDATE_DOCS=1 cargo test -p cyx reference` after changing them. -->

Every problem `cyx`, the Extensions view and the registry report has a stable code. The number never changes meaning, and a retired code is not reused. Each error says what is wrong, where, and what to change:

```text
error[CY0201]: net access to "api.vercel.com" is not declared
  --> codeyard-extension.toml:7:1
   = fix: add to [permissions]:  net = ["api.vercel.com"]
```

The fix line is the exact change when there is one; otherwise it is the general fix below. `cyx explain CY0201` prints a code's entry. Refused calls at run time are not errors of the manifest: they are `denied` with their own fix ([Commands](../commands.md#errors-and-refused-calls)).

## The manifest file

### CY0101

The manifest is not valid TOML, or a field has the wrong type or an unknown name.

**Fix:** correct the line shown; field names are kebab-case and listed in reference/manifest.md

### CY0102

The manifest is larger than the limit for `codeyard-extension.toml`.

**Fix:** shorten the manifest: move long text, such as setting descriptions, into the README

### CY0103

`extension.id` is not `publisher.name` in lowercase `a-z 0-9 -`.

**Fix:** use `<publisher>.<name>` in lowercase a-z, 0-9 and -, such as `you.midnight`

### CY0104

`extension.version` is not `major.minor.patch`.

**Fix:** write major.minor.patch, such as `0.1.0`; a pre-release adds a suffix, such as `1.0.0-beta.1`

### CY0105

A text field is empty or too long.

**Fix:** fill the field in and keep it within its length in reference/limits.md

### CY0106

A path is absolute, leaves the package or uses `\`.

**Fix:** use a relative path inside the extension's folder, with `/` between its parts

## Permissions

### CY0200

A `[permissions]` entry is malformed: a glob, host, program or command.

**Fix:** write workspace globs, host names without scheme, port or path (`api.example.com`), program names, and core commands as `core:<id>`

### CY0201

The extension uses a program, file or host that `[permissions]` does not declare.

**Fix:** add the entry to [permissions], or run `cyx check --fix`

### CY0202

The permissions are high risk and `[permissions] why` is missing.

**Fix:** add to [permissions] one honest sentence for the install sheet: why = "…"

## Contributions

### CY0300

A contribution is malformed: a field is missing, out of range or names something unknown.

**Fix:** change the entry as the message says; reference/contributions.md lists every field

### CY0301

Two contributions of one kind share an id.

**Fix:** give each entry of this kind its own id

### CY0302

A contribution runs extension code, but the manifest has no `[code]` table.

**Fix:** add [code] entry = "Cargo.toml" (or a .ts/.js file), or remove the contribution

## Activation

### CY0400

The manifest has `[code]`, but nothing would ever start it.

**Fix:** add a [[commands]], [[views]], provider or [[tasks]] entry, or [activation] workspace-contains = ["some-file"]; or remove [code]

## `when` expressions

### CY0500

A `when` or `enablement` expression does not parse or names an unknown key.

**Fix:** use ==, !=, &&, ||, ! and parentheses over the keys listed in developer-guide.md#the-manifest

## Dependencies

### CY0600

A dependency is malformed or names the extension itself.

**Fix:** name another extension by its full id with a minimum version: "other.ext" = "1.2"

## The component (`extension.wasm`)

### CY0700

`extension.wasm` is missing, is not a component, or imports or exports something it may not.

**Fix:** build extension.wasm with the Codeyard SDK: `cyx package`

### CY0701

The component uses an API version newer than this Codeyard.

**Fix:** update Codeyard, or rebuild the extension with the SDK of the API version this Codeyard has

## The registry index

### CY0801

A file of the registry index is larger than its limit.

**Fix:** nothing was installed; the registry's files are damaged or were changed on the way, so try again later

### CY0802

A file of the registry index is not valid JSON, or a field is missing, unknown or out of range.

**Fix:** nothing was installed; the registry's files are damaged or were changed on the way, so try again later

### CY0803

The registry index is written in a newer format than this Codeyard reads.

**Fix:** update Codeyard to read this registry

### CY0804

A file of the registry index is not signed by enough trusted keys.

**Fix:** nothing was installed; the registry's files are damaged or were changed on the way, so try again later

### CY0805

The registry's online signing key is past its expiry.

**Fix:** the registry has not renewed its signing key; try again later or update Codeyard

### CY0806

The registry served an older copy of its index than one already accepted.

**Fix:** an older copy of the registry was served (by a cache or an attacker); try again later

### CY0807

The registry index names another registry than its root.

**Fix:** check the registry's address in extensions.registries

## Publisher signatures

### CY0810

The signing roots pinned in the registry's root are unusable.

**Fix:** update Codeyard; the registry's root is not usable

### CY0811

The publisher signature is not a Sigstore bundle this version reads.

**Fix:** nothing was installed; the publisher should publish again with a current cyx

### CY0812

The publisher certificate does not chain to the pinned CA at the logged time, or has no publisher identity.

**Fix:** nothing was installed; the package was not signed by a known publisher

### CY0813

The transparency log record of the signature is missing or does not hold.

**Fix:** nothing was installed; the signature was not logged as it claims

### CY0814

The publisher signature is not over this package.

**Fix:** nothing was installed; the package is not the one the publisher signed
