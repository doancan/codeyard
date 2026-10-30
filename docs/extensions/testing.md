# Testing code extensions

There are two kinds of test, and a new extension from `cyx new command` or `cyx new view` comes with both set up:

- **Unit tests** with `cargo test` (Rust) or `npm test` (TypeScript, [below](#typescript)). They run on your computer, in milliseconds, against a mock Codeyard: fake files, fixture HTTP responses, recorded processes, scripted answers and a virtual clock.
- **`cyx test`** builds the real component and runs it in Codeyard's extension host: it activates the extension, runs every command and draws every view.

<!-- toc -->
- [Unit tests](#unit-tests)
- [Fixtures and scripted answers](#fixtures-and-scripted-answers)
- [What the test can check](#what-the-test-can-check)
- [Permissions in tests](#permissions-in-tests)
- [cyx test](#cyx-test)
- [TypeScript](#typescript)
- [In CI](#in-ci)
<!-- /toc -->

## Unit tests

The SDK compiles for your computer as well as for WebAssembly. On your computer every call goes to `MockHost` from `codeyard-extension-test`, so the code under test is exactly the code you ship.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension_test::MockHost;

    #[test]
    fn says_hello() {
        let host = MockHost::new();
        host.activate::<Hello>().unwrap();
        host.answer_input("Ada");
        host.command("hello", Value::Null).unwrap();
        assert_eq!(host.notices(), ["Hello, Ada!"]);
    }
}
```

`host.command(id, args)` runs a command and returns its result (or its error). Everything the command waits on is answered before it returns, so tests need no async runtime. The same goes for:

| Call | Does |
| --- | --- |
| `host.render("view")` | draws a view and returns its element tree |
| `host.view_event("view", json!(…))` | sends an event to a view |
| `host.format(doc, range, options)` | runs the formatter |
| `host.code_actions(doc, range, diagnostics)`, `host.run_code_action(id)` | asks the code action provider, runs a chosen action |
| `host.code_lenses(doc, range)`, `host.resolve_code_lens(doc, lens)`, `host.click_code_lens(&lens)` | asks for the lenses of the lines in `range`, resolves one, runs a lens's command with its arguments |
| `host.inlay_hints(doc, range)` | asks for the inlay hints of the lines in `range` |
| `host.change_setting(key, json)` | changes a setting and tells the extension |
| `host.advance(Duration::from_secs(90))` | moves the virtual clock, firing timers that fall due |
| `host.cancel_progress(id)` | presses Cancel on a progress notice |
| `host.deactivate::<Hello>()` | stops the extension |

Each test runs on its own thread with its own mock, so tests run in parallel without seeing each other.

## Fixtures and scripted answers

Set these up before or between calls. A path without a scheme is relative to the mock workspace folder, `file:///work`.

```rust
host.file("vercel.json", r#"{ "name": "web" }"#);
host.http("GET", "https://api.vercel.com/v6/deployments", 200, r#"{ "deployments": [] }"#);
host.process("vercel", &["ls"], ProcessFixture::ok("web  ready\n"));
host.active_document("src/main.rs", "rust", "fn main() {}");
host.setting("interval", Some("120"));
host.command_result("other.ext.list", "[1, 2]");

host.answer_input("Ada");        // the next input box
host.dismiss_input();
host.answer_pick(&[1]);          // the next quick pick
host.answer_notice(0);           // the next notice with buttons
```

Pickers and inputs without a scripted answer are dismissed. A request with no HTTP fixture, or a program with no process fixture, fails with an error that says which fixture to add, so a test never hangs or reaches the network.

## What the test can check

```rust
host.notices()            // notice texts, in order
host.fetched()            // every HTTP request sent
host.ran()                // every program started
host.read("out.txt")      // a file after the extension wrote it
host.storage()            // the key-value store
host.status_items()
host.diagnostics("src/main.rs")
host.invalidated()        // views asked to draw again
host.applied_edits()
host.timers(), host.progress(), host.logs(), host.clipboard(), host.terminal()
host.calls(), host.notes() // everything, as the SDK sent it
host.denied()             // every refused call
```

## Permissions in tests

`MockHost::new()` reads the `codeyard-extension.toml` next to your `Cargo.toml` and refuses what it does not allow, the way Codeyard does: a request to an undeclared host, a read outside the `read` globs, a program not in `run`. The refusal is the same `Error::Denied` the extension gets in Codeyard, with the fix:

```
denied: the manifest does not declare net = ["api.github.com"]
   = fix: add to [permissions]:  net = ["api.github.com"]
```

## Hot reload

`host.reload::<MyExtension>()` (TypeScript: `await host.reload(extension)`) does what `cyx dev` does with a new build: `deactivate`, forget everything registered, then `activate` with the reason `reload`. Storage is kept, and with it what the extension put in `ctx.state`:

```rust
host.command("count", Value::Null)?;
host.reload::<Counter>()?;
assert_eq!(host.command("count", Value::Null)?, json!(2));   // ctx.state() came along
```

`host.untrusted()` makes the mock behave like an untrusted folder (no sending, writing or running). `MockHost::allow_all()` allows everything, and `host.manifest(text)` checks against another manifest.

## cyx test

```sh
cyx test
```

```
you.hello 0.1.0
  ✓ activates (command:hello)
    notice: Hello, world! (1 so far)
  ✓ command hello answers
ok
```

`cyx test` builds `extension.wasm`, checks its imports against the manifest, and runs it in `codeyard-exthost`, the same extension host Codeyard uses, with the same memory and time limits. A scripted Codeyard answers the extension's calls: pickers and inputs are dismissed, there is no active document, and there is no network, no files and no programs (those calls fail with a message, which counts as an answer). Every call is first checked against the manifest's permissions.

It fails when:

- `activate` returns an error, or a command or view does not answer within 20 seconds;
- the extension traps (a panic, running out of memory, computing too long); the trap is printed with its frames;
- **any call is refused**, even if the extension handles the error. The refusal is printed with its fix:

```
error[CY0201]: net/fetch needs a permission the manifest does not declare
  --> codeyard-extension.toml
   = fix: add to [permissions]:  net = ["other.example.org"]
```

`cyx test` finds `codeyard-exthost` next to `cyx` (both ship with Codeyard), or uses `CODEYARD_EXTHOST`.

For a TypeScript extension, a thrown error's stack trace shows the TypeScript file and line, from the bundle's source map:

```
  ✓ command boom answers (with an error: it broke
    at explode (src/extension.ts:4:9)
    at activate/< (src/extension.ts:18:5))
```

A call to an interface the manifest grants nothing for (a file read without `read`) never reaches the extension host, because the component does not import it ([typescript.md](typescript.md#building-and-packaging)); the SDK refuses it with the same fix, and `cyx test` fails on it too.

## TypeScript

`@codeyard/extension/testing` is the same mock for `npm test`, which runs `test/*.test.ts` with Node's test runner. The extension runs through the same SDK code as in Codeyard.

```ts
import assert from "node:assert/strict";
import { test } from "node:test";

import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/extension.ts";

test("says hello", async () => {
  const host = new MockHost();
  await host.activate(extension);
  host.answerInput("Ada");
  assert.equal(await host.command("hello"), 1);
  assert.deepEqual(host.notices(), ["Hello, Ada! (1 so far)"]);
});
```

Everything is the Rust mock's, in camelCase, and the calls that drive the extension return Promises: `await host.command(id, args)`, `await host.render(view)`, `await host.viewEvent(view, event)`, `await host.changeSetting(key, value)`, `await host.advance(seconds)`. Fixtures take plain values: `host.http("GET", url, 200, { deployments: [] })` sends an object as JSON, `host.setting("interval", 120)`, `host.process("vercel", ["ls"], { stdout: "web  ready\n" })`. The checks are `host.notices()`, `host.fetched()`, `host.ran()`, `host.readText(path)`, `host.storage()`, `host.denied()` and the rest of the list above.

`new MockHost()` reads `codeyard-extension.toml` from the folder `npm test` runs in; `new MockHost({ manifest })` takes the text, and `MockHost.allowAll()` allows everything. While a mock is active it is the global `fetch`, so npm packages such as Octokit reach its fixtures and never the network; `host.dispose()` gives `fetch` back.

## In CI

`cyx new` writes a publish workflow; to also test on every push, add a job that runs the unit tests: `cargo test` for Rust (no WebAssembly target needed), or `npm ci && npm test` for TypeScript (Node.js 22.18 or newer). `cyx test` needs Codeyard's extension host, and for Rust the `wasm32-wasip2` target, so run it where Codeyard (or `codeyard/setup-cyx`) is installed.
