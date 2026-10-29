# TypeScript extensions

A code extension can be written in TypeScript (or JavaScript) with `@codeyard/extension`. `cyx` bundles it with esbuild and builds a WebAssembly component with ComponentizeJS, so it runs in the same extension host, with the same permissions and limits, as a Rust one. The API has the same shape as the Rust SDK ([commands.md](commands.md)); every call that reaches Codeyard returns a Promise. [testing.md](testing.md) covers tests, and [reference/api.md](reference/api.md) lists every call.

The SDK is **pre-release**: `@codeyard/extension` is not on npm yet (see [The SDK package](#the-sdk-package)). Its API may still change before 1.0.

<!-- toc -->
- [Setup](#setup)
- [Your first command](#your-first-command)
- [How calls work](#how-calls-work)
- [The API](#the-api)
- [fetch and npm packages](#fetch-and-npm-packages)
- [Errors and refused calls](#errors-and-refused-calls)
- [Tests](#tests)
- [Building and packaging](#building-and-packaging)
- [The SDK package](#the-sdk-package)
<!-- /toc -->

## Setup

You need Node.js 22.18 or newer. Nothing else: the first build installs esbuild and ComponentizeJS, at pinned versions, into `cyx`'s own cache folder (`CODEYARD_CYX_CACHE`, see [environment.md](../environment.md)), and reuses them after that.

## Your first command

```sh
cyx new command hello --lang ts      # or: cyx new view hello --lang ts
cd hello
npm install                          # the SDK, its types and TypeScript
npm test                             # unit tests against a mock Codeyard
cyx test                             # the built extension in the real extension host
cyx dev                              # Codeyard with the extension loaded
```

The manifest points at the main module:

```toml
[code]
entry = "src/extension.ts"

[[commands]]
id    = "hello"                 # becomes you.hello.hello
title = "Hello: Say Hello"
```

And `src/extension.ts` exports `activate`, which registers the command:

```ts
import type { Context } from "@codeyard/extension";

export function activate(ctx: Context) {
  ctx.command("hello", async (ctx) => {
    const name = (await ctx.input({ prompt: "Your name?" })) || "world";
    ctx.notice(`Hello, ${name}!`);
  });
}
```

`activate` runs once, the first time someone uses one of the extension's commands or views. Register handlers in it and return; `ctx.activationReason` says why it started. An optional `export function deactivate(ctx)` runs before the extension stops.

The complete templates, with their tests, are [examples/extensions/hello-command-ts](../../examples/extensions/hello-command-ts) and [examples/extensions/hello-view-ts](../../examples/extensions/hello-view-ts). [examples/extensions/octokit-repos-ts](../../examples/extensions/octokit-repos-ts) uses npm packages.

## How calls work

A call that reaches Codeyard (a notice with buttons, a file read, an HTTPS request) returns a Promise: `await ctx.readText(uri)`. Calls Codeyard answers on the spot (storage, settings, logging, diagnostics, status items) return their value directly.

Under the hood every such call returns a ticket at once, and the SDK settles the Promise when Codeyard delivers the result. The extension never blocks while it waits, so it can answer other requests at the same time. Two rules are enough:

- **Don't block.** A handler that computes for more than 2 seconds without awaiting is stopped. Poll with `ctx.every`, not with timers of your own.
- **Answer fast.** Providers (views, formatters, code actions) are cancelled when the user moves on; the answer of a cancelled request is dropped.

`ctx.background(promise)` runs work that no request waits for, such as work started from `activate`; a failure is logged with its stack. A handler may also return a value directly instead of a Promise.

## The API

The same calls as the Rust SDK, in camelCase:

| Rust | TypeScript |
| --- | --- |
| `ctx.command(id, async \|ctx, args\| …)` | `ctx.command(id, async (ctx, args) => …)`; the return value is sent as JSON |
| `ctx.view(id, …)`, `ctx.on_view_event(id, …)`, `ctx.invalidate(id)` | `ctx.view(id, async (ctx) => tree)`, `ctx.onViewEvent(id, …)`, `ctx.invalidate(id)` |
| `ctx.formatter`, `ctx.code_actions`, `ctx.on_code_action`, `ctx.on_uri` | `ctx.formatter`, `ctx.codeActions`, `ctx.onCodeAction`, `ctx.onUri` |
| `ctx.every(seconds, …)` → `Timer` | `ctx.every(seconds, async (ctx) => …)` → `Timer` with `clear()` |
| `ctx.notice`, `ctx.notify`, `ctx.quick_pick`, `ctx.quick_pick_many`, `ctx.input` | `ctx.notice`, `ctx.notify`, `ctx.quickPick`, `ctx.quickPickMany`, `ctx.input` |
| `ctx.progress(title, cancellable)` | `ctx.progress(title)`, or `await ctx.withProgress(title, async (p) => …)`, which ends it |
| `ctx.storage().get_json` / `set_json` | `ctx.storage.getJson` / `setJson` (also `get`, `set`, `getText`, `delete`, `keys`) |
| `ctx.setting::<T>(key)`, `ctx.on_settings_changed` | `ctx.setting<T>(key)`, `ctx.onSettingsChanged` |
| `ctx.active_document`, `ctx.selections`, `ctx.apply_edits`, `ctx.set_diagnostics`, `ctx.set_decorations` | `ctx.activeDocument`, `ctx.selections`, `ctx.applyEdits`, `ctx.setDiagnostics`, `ctx.setDecorations` |
| `ctx.read_file`, `ctx.read_text`, `ctx.stat`, `ctx.find_files`, `ctx.write_file` | `ctx.readFile`, `ctx.readText`, `ctx.stat`, `ctx.findFiles`, `ctx.search`, `ctx.writeFile` |
| `ctx.fetch`, `ctx.fetch_as`, `ctx.session` | the global `fetch` (or `ctx.fetch`), `fetch(url, { auth: "github" })`, `ctx.authFetch("github")`, `ctx.session` |
| `ctx.spawn`, `ctx.run` | `ctx.spawn(command)` (`for await (const chunk of process)`, `await process.wait()`), `ctx.run(command)` |
| `ctx.secret`, `ctx.read_clipboard`, `ctx.send_to_terminal` | `ctx.secret`, `ctx.readClipboard`, `ctx.sendToTerminal` |

Quick picks answer with the index of the item picked (or `undefined` when dismissed), as in Rust. A view's tree is a plain object: `{ type: "column", children: [{ type: "text", text: "3 clicks" }, { type: "button", key: "add", label: "Add one" }] }` ([commands.md](commands.md#views) has the elements). `console.log` goes to the extension's log, as `ctx.log` does.

## fetch and npm packages

The global `fetch` is the SDK's, mapped to Codeyard's `net`: the request leaves from the machine Codeyard runs on, to a host the manifest lists. The Promise resolves when the response head arrives; the body streams in (`response.body`, or `for await (const chunk of response.chunks())`), and `json()`, `text()`, `bytes()` and `arrayBuffer()` read it whole. `signal` cancels a request. One option is Codeyard's own: `auth` signs the request for an account, and the extension never sees the token.

```ts
const res = await fetch("https://api.github.com/user/repos", { auth: "github" });   // auth = ["github"]
```

Because `fetch` is global, npm packages that use it work unchanged once bundled: Octokit, zod, ky and API clients built on `fetch`. Give a client that takes its own `fetch` the signed one:

```ts
import { Octokit } from "@octokit/core";

const octokit = new Octokit({ request: { fetch: ctx.authFetch("github") } });
const { data } = await octokit.request("GET /user/repos");
```

Packages for Node.js do not work: there is no `fs`, `child_process`, `net` or `http`. The build stops at the import that needs one:

```
error: "node:fs" is a Node.js built-in module, which Codeyard extensions do not have
  --> src/extension.ts:1:30
   = fix: use ctx.readFile / ctx.writeFile for workspace files, fetch for HTTP and ctx.run for programs; pick npm packages that work in browsers
```

esbuild picks each package's browser build where it has one. WebSockets are not offered in v1.

## Errors and refused calls

A refused call rejects with a `DeniedError`, whose message says what to add to the manifest:

```
denied: the manifest does not declare net = ["api.github.com"]
   = fix: add to [permissions]:  net = ["api.github.com"]
```

`e.permission` is the manifest line and `e.why` one of `not-declared`, `revoked-by-user`, `untrusted-workspace`, `revoked-extension`. Other errors from Codeyard are `CodeyardError`s with `e.kind` (`not-found`, `cancelled`, `timeout`, `invalid`, `other`).

A command that throws or rejects answers with the error's message and stack trace. Stack traces name the bundle (`extension.js:120:9`); `cyx test` shows the TypeScript file and line instead (`src/extension.ts:12:5`), from the source map `cyx` writes next to `extension.wasm`. The map (`extension.js.map`) is packaged with the extension, so Codeyard can map traces the same way.

## Tests

`npm test` runs the tests in `test/` with Node's built-in test runner, against the mock host from `@codeyard/extension/testing`. Node runs the `.ts` files as they are: no compile step.

```ts
import assert from "node:assert/strict";
import { test } from "node:test";

import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/extension.ts";

test("says hello", async () => {
  const host = new MockHost();             // checks calls against ./codeyard-extension.toml
  await host.activate(extension);
  host.answerInput("Ada");
  await host.command("hello");
  assert.deepEqual(host.notices(), ["Hello, Ada!"]);
});
```

The mock has the same fixtures and checks as the Rust one: files, HTTP responses (`host.http("GET", url, 200, { … })`), processes, scripted answers, a virtual clock (`await host.advance(90)`), and every refused call with its fix. While a mock is active it is the global `fetch`, so npm packages reach its fixtures and never the network. [testing.md](testing.md#typescript) lists it all, and `cyx test` works the same as for Rust.

## Building and packaging

`cyx package`, `cyx test` and `cyx dev` build the component when `[code] entry` ends in `.ts`, `.js` or `.mjs`:

1. esbuild bundles the entry module, its imports and `node_modules` into one ES module (`extension.js`, kept in the cache folder), with a source map.
2. ComponentizeJS turns it into `extension.wasm`, against the `codeyard:api` world narrowed to the interfaces the manifest grants. A component built this way imports every interface of its world, so a narrow world is what keeps an extension without `net` from importing `net`. A call to an interface that is not there rejects with a `DeniedError` and its fix, and `cyx test` fails on it like on any refused call.
3. The import check runs, as for Rust.

The component is about 12 MB, most of it the JavaScript engine every TypeScript extension embeds (the package compresses it to about 4 MB). `cyx` always bundles its own copy of `@codeyard/extension`, the one that matches the API it builds for; the copy in `node_modules` is for `npm test` and your editor.

## The SDK package

`@codeyard/extension` lives in [sdk/typescript](../../sdk/typescript/README.md) in the Codeyard repository and ships as TypeScript sources. Until it is on npm, create extensions with `--sdk` pointing at a Codeyard checkout, which writes a `file:` dependency on its `sdk/typescript`:

```sh
cyx new command hello --lang ts --sdk ~/src/codeyard
```

Without `--sdk` the template asks npm for `@codeyard/extension` 0.1.0, which `npm install` cannot find yet; `cyx test` and `cyx package` work either way, because `cyx` bundles its own copy.
