# Migrating to API 1.0

`codeyard:api` 1.0.0 is the first stable version of the extension API. It ships with Codeyard 0.7.0, the Rust SDK
`codeyard-extension` of the same release and the TypeScript SDK `@codeyard/extension` 1.0.0. This page is for
extensions built on API 0.1.

Most extensions need one thing: **rebuild with the new SDK** (`cyx package`). The WIT has the same shape as 0.1, so
code that compiles today keeps working. The few source changes are listed below.

## What 1.0 promises

- **Minors only add.** From 1.0, a minor version (1.1, 1.2, …) adds interfaces, functions and types. Nothing that
  shipped in 1.0 is removed or changed until 2.0. A test in Codeyard compares the WIT with the released copy and fails
  on any removal or change (`crates/ext-api/released`).
- **Every item carries `@since`.** The [API reference](reference/api.md) says which version added each interface,
  function and type.
- **`cyx` works out the oldest Codeyard you need** from the API items your code uses. `cyx check` prints it, for
  example "It needs Codeyard 0.7.0 or newer (API 1.0.0, …)". An extension built with a newer SDK still runs on an
  older 1.x Codeyard when it uses only items that Codeyard has. For TypeScript, every function of the interfaces your
  manifest grants counts, because the build imports them all.
- **A major stays supported for 24 months** after the next one ships.

Because the component model has no optional fields or cases, a minor never adds a field to a record or a case to a
variant or enum. New data comes as new types and functions.

## Components built against 0.1

Codeyard 0.7.0 still runs an `extension.wasm` built against API 0.1. The two versions have the same shape, so the
extension host renames the component's `codeyard:api/…@0.1.0` imports to `@1.0.0` before it compiles it. You do not
have to republish at once.

During this transition:

- `cyx check` and `cyx package` warn: "extension.wasm was built against API 0.1.0 …".
- `cyx publish` and the registry refuse a 0.1 build, so every new version is a 1.0 build.
- Codeyard keeps running 0.1 builds for at least six months after 0.7.0. The release before the one that drops them
  says so in its changelog. After that, installing a 0.1 build fails with `CY0700`, and the fix points here.

Any other 0.x version (there was none before 0.1) is refused with `CY0700`.

## Changes in your code

### Both SDKs

- **Rebuild.** Rust: update `codeyard-extension` (and `codeyard-extension-test`) and run `cyx package`. TypeScript:
  set `"@codeyard/extension": "1.0.0"` in `package.json` (or keep the `file:` link to a checkout), run `npm install`,
  then `cyx package`. `cyx` builds TypeScript with its own copy of the SDK, so the package is a 1.0 build either way.
- **Views use one format.** Views written with the `{ "type": "column", … }` shorthand of the old `cyx new view`
  templates were never drawn: Codeyard refused them with "unknown field `type`". Build trees with the SDK's `ui`
  functions (`ui::column()`, `ui.column([...])`) or write the form in [reference/ui.md](reference/ui.md). A view
  event is `{"click": {"path": ["add"]}}`, not `{ click: "add" }`. Both mock hosts and `cyx test` now check trees the
  way Codeyard does. See [views.md](views.md).
- **`denied` errors keep the host's text.** For an `auth.fetch` to a host outside an `[[oauth]]` account, the
  permission is now `[[oauth]] <id> hosts = ["<host>"]` and the fix says to add the host to that table. The SDKs fill
  in a manifest line only when Codeyard sends a bare permission name (`net`, `auth`). If your code compares
  `Denied::permission` / `DeniedError.permission` with a string, check it against the new text.
- **`remote.account` is opaque.** A `[[pull-requests]]` remote on a self-managed instance carries
  `<id>@<host>` as its account. Pass it to `fetch_as` / `fetchAs` / `auth.fetch` as it is; do not parse it or compare
  it with your provider id. See [self-managed instances](developer-guide.md#call-an-api-with-the-users-github-login).

### Rust

- `ctx.state().remove(key)` is now `ctx.state().delete(key)`, as in `ctx.storage()` and the TypeScript SDK. New:
  `ctx.state().keys()`.

### TypeScript

Nothing else changes in the API.

## Checklist

1. Update the SDK dependency (above).
2. Replace view shorthand trees with `ui` builders, and fix view event tests to send `{"click": {"path": [...]}}`.
3. Rust: rename `ctx.state().remove` to `delete`.
4. Run your unit tests, then `cyx test`.
5. `cyx check`: no "built against API 0.1.0" warning, and note the Codeyard version it prints.
6. `cyx publish`.
