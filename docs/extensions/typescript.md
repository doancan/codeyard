# TypeScript extensions

A code extension can be written in TypeScript (or JavaScript) with `@codeyard/extension`. `cyx` bundles it with esbuild and builds a WebAssembly component with ComponentizeJS, so it runs in the same extension host, with the same permissions and limits, as a Rust one. The API has the same shape as the Rust SDK ([commands.md](commands.md)); every call that reaches Codeyard returns a Promise. [testing.md](testing.md) covers tests, and [reference/api.md](reference/api.md) lists every call.

`@codeyard/extension` is not on npm yet (see [The SDK package](#the-sdk-package)). It implements API 1.0, which later 1.x versions only add to ([Migrating to API 1.0](migrating-to-1.0.md)).

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

`activate` may be `async` (`export async function activate(ctx)`), for instance to read a configuration file first. Requests that arrive before its Promise settles wait for it, so a handler it registers after an `await` is found. If the Promise rejects, the activation fails: the extension's log shows `activate failed: …`, every request answers with that error, and `cyx test` and the mock host fail. Register handlers before the first `await` when they do not depend on it, so they work even when the rest fails.

The complete templates, with their tests, are [examples/extensions/hello-command-ts](../../examples/extensions/hello-command-ts) and [examples/extensions/hello-view-ts](../../examples/extensions/hello-view-ts). [examples/extensions/octokit-repos-ts](../../examples/extensions/octokit-repos-ts) uses npm packages.

## How calls work

A call that reaches Codeyard (a notice with buttons, a file read, an HTTPS request) returns a Promise: `await ctx.readText(uri)`. Calls Codeyard answers on the spot (storage, settings, logging, diagnostics, status items) return their value directly.

Under the hood every such call returns a ticket at once, and the SDK settles the Promise when Codeyard delivers the result. The extension never blocks while it waits, so it can answer other requests at the same time. Two rules are enough:

- **Don't block.** A handler that computes for more than 2 seconds without awaiting is stopped. Poll with `ctx.every`, not with timers of your own.
- **Answer fast.** Providers (views, formatters, code actions, hover, completion, code lenses, inlay hints) are cancelled when the user moves on; the answer of a cancelled request is dropped.

`ctx.background(promise)` runs work that no request waits for, such as work started from `activate`; a failure is logged with its stack. A handler may also return a value directly instead of a Promise.

## The API

The same calls as the Rust SDK, in camelCase:

| Rust | TypeScript |
| --- | --- |
| `ctx.command(id, async \|ctx, args\| …)` | `ctx.command(id, async (ctx, args) => …)`; the return value is sent as JSON |
| `ctx.view(id, …)`, `ctx.on_view_event(id, …)`, `ctx.invalidate(id)` | `ctx.view(id, async (ctx) => tree)`, `ctx.onViewEvent(id, …)`, `ctx.invalidate(id)` |
| `ctx.formatter`, `ctx.code_actions`, `ctx.on_code_action`, `ctx.on_uri` | `ctx.formatter`, `ctx.codeActions`, `ctx.onCodeAction`, `ctx.onUri` |
| `ctx.hover(async \|ctx, doc, position\| …)`, `ctx.completion(async \|ctx, doc, position, trigger\| …)` | `ctx.hover(async (ctx, doc, position) => ({ contents: "…" }))`, `ctx.completion(async (ctx, doc, position, trigger) => [{ label: "…" }])` (an array, or `{ items, incomplete }`) |
| `ctx.code_lenses`, `ctx.resolve_code_lens`, `ctx.inlay_hints` | `ctx.codeLenses`, `ctx.resolveCodeLens`, `ctx.inlayHints`: a lens is `{ range, command: { title, command, args } }` |
| `ctx.every(seconds, …)` → `Timer` | `ctx.every(seconds, async (ctx) => …)` → `Timer` with `clear()` |
| `ctx.notice`, `ctx.notify`, `ctx.quick_pick`, `ctx.quick_pick_many`, `ctx.input` | `ctx.notice`, `ctx.notify`, `ctx.quickPick`, `ctx.quickPickMany`, `ctx.input` |
| `ctx.progress(title, cancellable)` | `ctx.progress(title)`, or `await ctx.withProgress(title, async (p) => …)`, which ends it |
| `ctx.storage().get_json` / `set_json` | `ctx.storage.getJson` / `setJson` (also `get`, `set`, `getText`, `delete`, `keys`) |
| `ctx.state().get` / `set` / `delete` / `keys` | `ctx.state.get` / `set` / `delete` / `keys`: in memory, kept across a hot reload in `cyx dev` |
| `ctx.setting::<T>(key)`, `ctx.on_settings_changed` | `ctx.setting<T>(key)`, `ctx.onSettingsChanged` |
| `ctx.active_document`, `ctx.selections`, `ctx.apply_edits`, `ctx.set_diagnostics`, `ctx.set_decorations` | `ctx.activeDocument`, `ctx.selections`, `ctx.applyEdits`, `ctx.setDiagnostics`, `ctx.setDecorations` |
| `ctx.read_file`, `ctx.read_many`, `ctx.read_text`, `ctx.stat`, `ctx.find_files`, `ctx.search` | `ctx.readFile`, `ctx.readMany`, `ctx.readText`, `ctx.stat`, `ctx.findFiles`, `ctx.search` |
| `ctx.write_file`, `ctx.delete_file`, `ctx.rename`, `ctx.create_directory` | `ctx.writeFile`, `ctx.deleteFile`, `ctx.rename`, `ctx.createDirectory` |
| `ctx.open_external`, `ctx.execute` | `ctx.openExternal`, `ctx.execute` (a core command needs `permissions.commands`) |
| `ctx.fetch`, `ctx.fetch_as`, `ctx.session` | the global `fetch` (or `ctx.fetch`), `fetch(url, { auth: "github" })`, `ctx.authFetch("github")`, `ctx.session` |
| `ctx.spawn`, `ctx.run` | `ctx.spawn(command)` (`for await (const chunk of process)`, `await process.wait()`), `ctx.run(command)` |
| `ctx.secret`, `ctx.set_secret`, `ctx.delete_secret` | `ctx.secret`, `ctx.setSecret`, `ctx.deleteSecret`; ask for a secret with `ctx.input({ prompt, password: true })` |
| `ctx.read_clipboard`, `ctx.send_to_terminal` | `ctx.readClipboard`, `ctx.sendToTerminal` |

Quick picks answer with the index of the item picked (or `undefined` when dismissed), as in Rust. A view's tree is built with `ui`, as in Rust: `ui.column([ui.text("3 clicks"), ui.button("add", "Add one")])` (see [Views](views.md#build-it-with-the-sdk)). It is a plain `ViewNode` object in the form of the [views reference](reference/ui.md), typed per element, so a hand-written tree type-checks too. `console.log` goes to the extension's log, as `ctx.log` does.

## Pull request providers

A `[[pull-requests]]` entry in the manifest makes the extension a pull (merge) request provider for the remotes on its `hosts`. Register it in `activate` with the entry's `id`:

```toml
[permissions]
auth = ["gitea"]

[[pull-requests]]
id    = "gitea"
label = "Gitea"
hosts = ["codeberg.org"]
auth  = "gitea"
```

```ts
ctx.pullRequests("gitea", {
  async list(ctx, remote) {
    const r = await ctx.fetchAs(remote.account!, `https://${remote.host}/api/v1/repos/${remote.path}/pulls?state=open`);
    return (await r.json()).map((p: any) => ({
      id: String(p.number),
      reference: `#${p.number}`,
      title: p.title,
      author: p.user.login,
      state: p.draft ? "draft" : "open",
      sourceBranch: p.head.ref,
      targetBranch: p.base.ref,
      updated: Date.parse(p.updated_at),
      url: p.html_url,
    }));
  },
  details: async (ctx, remote, id) => ({ summary: await pull(ctx, remote, id), description: "…", checks: [], labels: [] }),
  files: async (ctx, remote, id) => [{ path: "src/a.ts", status: "modified" }],
  contents: async (ctx, remote, id, file) => ({ base: "…", head: "…" }),
  comments: async (ctx, remote, id) => [],
  addComment: async (ctx, remote, id, body) => {},
  checkout: async (ctx, remote, id) => ({ ref: `refs/pull/${id}/head`, branch: `pr-${id}` }),
});
```

`remote` is a `Remote`: the remote's `name`, `url`, `host`, `path` (such as `group/app`) and `account`, the manifest's `auth` provider. Call the forge with `ctx.fetchAs(remote.account, …)`: Codeyard signs the request, and the extension never sees the token. A request is signed only once the user has signed in, so call `ctx.session(remote.account)` first; it asks on first use and returns at once after that. `list`, `details`, `files` and `contents` are required. `comments` is optional: without it a request has no comments. Without `addComment` or `checkout`, Codeyard refuses those actions. Times (`updated`, `created`) are Unix milliseconds; leave them out when unknown.

In tests, `host.listPullRequests(remote)`, `host.pullRequestDetails(remote, id)`, `host.pullRequestFiles`, `host.pullRequestContents`, `host.pullRequestComments`, `host.addPullRequestComment` and `host.checkoutPullRequest` send the queries, and `host.http(…)` answers the signed requests. `host.fetched()` shows each request with the `auth` provider that signed it. As in Codeyard, the mock refuses a signed request to a host outside the provider's `[[oauth]] hosts`, and one that sets its own `authorization` header. [examples/extensions/gitlab](../../examples/extensions/gitlab) is a complete provider for GitLab in TypeScript.

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

Without `--sdk` the template asks npm for `@codeyard/extension` 1.0.0, which `npm install` cannot find yet; `cyx test` and `cyx package` work either way, because `cyx` bundles its own copy.
