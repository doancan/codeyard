# Testing code extensions

There are two kinds of test, and a new extension from `cyx new command`, `cyx new view` or `cyx new pull-requests` comes with both set up:

- **Unit tests** with `cargo test` (Rust) or `npm test` (TypeScript, [below](#typescript)). They run on your computer, in milliseconds, against a mock Codeyard: fake files, fixture HTTP responses, recorded processes, scripted answers and a virtual clock.
- **`cyx test`** builds the real component and runs it in Codeyard's extension host: it activates the extension, runs every command, draws every view and asks every provider once, checking each answer as Codeyard does.

Both check every call with Codeyard's own rules (the `ext-manifest` crate, and for TypeScript a copy tested against it): the same globs, paths, URLs, status bar rule and size limits. A test that passes is a call Codeyard makes.

<!-- toc -->
- [Unit tests](#unit-tests)
- [Fixtures and scripted answers](#fixtures-and-scripted-answers)
- [What the test can check](#what-the-test-can-check)
- [Permissions in tests](#permissions-in-tests)
- [Accounts in tests](#accounts-in-tests)
- [Watches and servers in tests](#watches-and-servers-in-tests)
- [cyx test](#cyx-test)
- [Fixtures for cyx test](#fixtures-for-cyx-test)
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
| `host.render("view")` | draws a view and returns its element tree, checked as Codeyard checks it |
| `host.view_event("view", json!(…))` | sends an event to a view, in the form Codeyard sends: `json!({"click": {"path": ["add"]}})` |
| `host.format(doc, range, options)` | runs the formatter |
| `host.code_actions(doc, range, diagnostics)`, `host.run_code_action(id)` | asks the code action provider, runs a chosen action |
| `host.hover(doc, position)`, `host.completion(doc, position, trigger)` | asks the hover or completion provider (`trigger`: `Some(".")` for a trigger character) |
| `host.code_lenses(doc, range)`, `host.resolve_code_lens(doc, lens)`, `host.click_code_lens(&lens)` | asks for the lenses of the lines in `range`, resolves one, runs a lens's command with its arguments |
| `host.inlay_hints(doc, range)` | asks for the inlay hints of the lines in `range` |
| `host.change_setting(key, Some("30"))` | changes a setting (its JSON text; `None` resets it) and tells the extension |
| `host.advance(Duration::from_secs(90))` | moves the virtual clock, firing timers that fall due |
| `host.cancel_progress(id)` | presses Cancel on a progress notice |
| `host.deactivate::<Hello>()` | stops the extension |

`render` checks the tree with Codeyard's own code (crates/ext-ui), so a tree Codeyard would refuse fails the test
with the reason Codeyard would show in place of the view, for example ``Codeyard would not draw view main: the view is
malformed: unknown field `type` ``. `view_event` refuses an event Codeyard never sends, such as `{"click": "add"}`. The
tree comes back as JSON in the form of the [views reference](reference/ui.md):
`host.render("main")?.pointer("/children/1/element/text/text")`.

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
host.notices();           // notice texts, in order
host.fetched();           // every HTTP request sent
host.ran();               // every program started
host.read("out.txt");     // a file after the extension wrote it
host.storage();           // the key-value store
host.status_items();
host.diagnostics("src/main.rs");
host.decorations("src/main.rs");  // by decoration set
host.invalidated();       // views asked to draw again
host.applied_edits();
host.secrets();           // the keychain entries (`secrets`), as the extension left them
host.timers(); host.progress(); host.logs(); host.clipboard(); host.terminal();
host.calls(); host.notes();   // everything, as the SDK sent it
host.denied();            // every refused call
```

## Permissions in tests

`MockHost::new()` reads the `codeyard-extension.toml` next to your `Cargo.toml` and refuses what it does not allow, the way Codeyard does: a request to an undeclared host, a read outside the `read` globs, a path outside the workspace folder (`src/../../etc/passwd`, `%2e%2e` included), a program not in `run`, an `auth.fetch` to a host that is not one of the account's (`[[oauth]] hosts`, or GitHub's API hosts). The refusal is the same `Error::Denied` the extension gets in Codeyard, with the fix:

```
denied: the manifest does not declare net = ["api.github.com"]
   = fix: add to [permissions]:  net = ["api.github.com"]
```

An `auth.fetch` to a host that is not one of the account's names that host and the `[[oauth]]` table it belongs in:

```
denied: the manifest does not declare [[oauth]] gitlab hosts = ["evil.example.org"]
   = fix: add "evil.example.org" to hosts in the [[oauth]] table with id = "gitlab"
```

Globs mean what they mean in Codeyard ([globs](reference/globs.md)): `read = ["main.rs"]` is the file at the top of the folder, not `src/main.rs`. The other rules are Codeyard's too:

- A URL Codeyard never fetches (`http://`, a user name or password in it) fails with `invalid`, even when a fixture exists for it. One that names an IP address, and a path outside the workspace, are denied with `(cannot be granted: …)` and the fix `none: this cannot be granted, …`, since no manifest entry allows them.
- `set_status_item` refuses an id the manifest does not declare in `[[status-bar]]` and a text that is empty or longer than 64 characters.
- An answer (a command's result, a view's tree) over 8 MiB fails its request with `invalid: the result is 20 MiB; at most 8 MiB`.
- `open_external` works only while the extension handles a user action (a command, a view event), not from a provider or a timer.

`on_settings_changed` and `every` handlers may return `Result<()>`, so `?` works in them; an error goes to the extension's log (`host.logs()`).

## Accounts in tests

Every `auth` provider starts signed in as `test-user`. To test what the extension does when the sign-in does not work out:

```rust
host.decline_sign_in("gitlab");   // the user picks Cancel: auth.session fails with `cancelled`
host.signed_out("gitlab");        // after Sign Out: auth.fetch fails with "not signed in to GitLab; call auth.session …",
                                  // and the next auth.session signs in again
host.sign_in("gitlab", "ada");    // signed in, with this account name
host.instance("gitlab", "git.example.com");  // a self-managed instance the user added: account `gitlab@git.example.com`
```

TypeScript: `host.declineSignIn(provider)`, `host.signedOut(provider)`, `host.signIn(provider, label)`, `host.instance(provider, host)`. A token the service refuses is its answer, so give the request a `401` fixture.

## Watches and servers in tests

Watches (`ctx.on_documents`, `ctx.on_files`) get events when the test changes documents and files. Like Codeyard, the
mock sends them only for paths the manifest's `read` allows, a file event only when the watch's glob matches, and
`opened` for the documents already open when a document watch starts:

```rust
host.open_document("src/a.rs", "rust", "fn a() {}");   // document watches get `opened`
host.edit_document("src/a.rs", "fn a() {} // TODO"); // `changed`, version 2
host.save_document("src/a.rs");                       // `saved`
host.create_file("src/b.rs", "fn b() {}");            // file watches whose glob matches get `created`
host.change_file("src/b.rs", "fn b() { 1 }");
host.delete_file("src/b.rs");
host.allow_host("github.example.com");                // the user allowed this server for `net-from-setting`
```

Until `allow_host`, a request from an extension with `net-from-setting` to a host outside `net` is refused with
`net-from-setting host = "github.example.com"`, whose fix says the user allows it on the extension page.

TypeScript: `await host.openDocument(path, language, text)`, `await host.editDocument(path, text)`,
`await host.saveDocument(path)`, `await host.createFile(path, contents)`, `await host.changeFile(path, contents)`,
`await host.deleteFile(path)` and `host.allowHost(host)`.

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

`cyx test` builds `extension.wasm`, checks its imports against the manifest, and runs it in `codeyard-exthost`, the same extension host Codeyard uses, with the same memory and time limits. A scripted Codeyard answers the extension's calls: pickers and inputs are dismissed, there is no active document, files and HTTP responses come only from [the fixtures](#fixtures-for-cyx-test), and there are no programs. Every call is first checked with Codeyard's rules and the manifest's permissions.

It runs, in order:

- every command, with no arguments unless the fixtures give some;
- every view, drawn once;
- every provider once (formatter, code actions, hover, completion, code lenses, inlay hints, tasks, source control), with a sample document: the first file of the fixtures' `files` (in the provider's first language), or an empty one;
- every `[[pull-requests]]` provider, asked for the open requests of a remote on its first host, `https://<host>/example/repo.git`, signed with its `auth` account.

An error is an answer only when the handler first asked for something `cyx test` cannot give offline: an HTTP request without a fixture, a file the fixtures do not have, a program. Any other error fails the test: an exception, a command that is declared but not registered (`no command 'x' is registered`), an answer over 8 MiB. After a trap or a request left unanswered, `cyx test` starts the extension host again and goes on, so one run reports every problem.

It fails when:

- `activate` returns an error, or an `async activate` rejects after an `await` (the log shows `activate failed: …`);
- a command, view or provider fails as above, or does not answer within 20 seconds;
- a view draws a tree Codeyard would refuse; the reason is printed as Codeyard shows it;
- the extension traps (a panic, running out of memory, computing too long); the trap is printed with the extension's own frames first (the Rust runtime's panic handling is left out, names are demangled);
- **any call is refused**, even if the extension handles the error. The refusal is printed with its fix:

```
error[CY0201]: net/fetch needs a permission the manifest does not declare
  --> codeyard-extension.toml
   = fix: add to [permissions]:  net = ["other.example.org"]
```

`cyx test` finds `codeyard-exthost` next to `cyx` (both ship with Codeyard), or uses `CODEYARD_EXTHOST`.

Each watch the extension starts gets a file of its own, named after its glob (or the first `read` glob):
a document watch sees it opened, changed and saved, a file watch sees it created, changed and deleted. A trap in a
handler fails the test. For `net-from-setting`, the host in the setting's default stands in for one the user allowed.

## Fixtures for cyx test

`cyx test` reaches no network and no files. To run the success path of a command or provider that fetches or reads,
give it answers in `test/fixtures.json` (or another file with `cyx test --fixtures FILE`):

```json
{
  "http": [
    {
      "method": "GET",
      "url": "https://gitlab.com/api/v4/projects/example%2Frepo/merge_requests?state=opened",
      "status": 200,
      "body": [{ "iid": 7, "title": "Fix the parser" }]
    },
    { "method": "*", "url": "https://api.example.com/down", "status": 503, "body": "down", "headers": { "retry-after": "60" } }
  ]
}
```

- `url` matches the request's URL in full, query included; `method` is a method or `*` (the default).
- `status` defaults to 200. A `body` that is a string is sent as it is; any other JSON value is sent as JSON, with
  `content-type: application/json` unless `headers` sets one.
- The answer is for `net.fetch` and `auth.fetch` alike, after the usual checks: a host not in `net`, or not one of the
  account's hosts, is still refused and fails the test. An `auth.fetch` is answered as if the account were signed in.
- A request without a fixture fails with a message naming the URL and the file, which counts as an answer.
- A URL Codeyard never fetches (`http://`, `https://user:pw@…`, an IP address) is refused in the file itself.

Workspace files are text by path, relative to the extension's folder (which `cyx test` opens as the workspace). Reads
are checked against `read` first, so `.git/config` needs `read = [".git/config"]`, as in Codeyard:

```json
{
  "files": {
    ".git/config": "[remote \"origin\"]\n\turl = https://github.com/you/app.git\n",
    "src/main.rs": "fn main() {}\n"
  }
}
```

`find-files` lists the fixture files its glob matches and `read` allows. The first file is also the sample document the
providers are asked about.

A command that needs arguments gets them from `commands`, by its local id; the value is any JSON:

```json
{ "commands": { "collect": "TODO: write the docs" } }
```

The `test/` and `tests/` folders at the top of an extension are left out of its package, so fixtures and tests never
ship.

For a TypeScript extension, a thrown error's stack trace shows the TypeScript file and line, from the bundle's source map:

```
  ✗ command boom failed: it broke
    at explode (src/extension.ts:4:9)
    at activate/< (src/extension.ts:18:5)
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
  host.dismissInput();
  assert.equal(await host.command("hello"), 2);
  assert.deepEqual(host.notices(), ["Hello, Ada! (1 so far)", "Hello, world! (2 so far)"]);
});
```

Everything is the Rust mock's, in camelCase, and the calls that drive the extension return Promises: `await host.command(id, args)`, `await host.render(view)`, `await host.viewEvent(view, event)`, `await host.changeSetting(key, value)`, `await host.advance(seconds)`, `await host.hover(doc, position)`,
`await host.completion(doc, position, trigger)`. Fixtures take plain values: `host.http("GET", url, 200, { deployments: [] })` sends an object as JSON, `host.setting("interval", 120)`, `host.process("vercel", ["ls"], { stdout: "web  ready\n" })`. The checks are `host.notices()`, `host.fetched()`, `host.ran()`, `host.readText(path)`, `host.storage()`, `host.denied()` and the rest of the list above.

`render` and `viewEvent` check trees and events as the Rust mock does. The TypeScript mock cannot call Codeyard's code,
so it checks with a copy of its rules generated from crates/ext-ui; a test in Codeyard's repository runs both on the
same few hundred trees and requires the same verdicts.

The rules on paths, URLs, status items, answer sizes and links are Codeyard's, as in the Rust mock ([Permissions in tests](#permissions-in-tests)); the mock has a copy of them, and a test in Codeyard's repository runs it on the cases Codeyard's own rules judged.

`new MockHost()` reads `codeyard-extension.toml` from the folder `npm test` runs in; `new MockHost({ manifest })` takes the text, and `MockHost.allowAll()` allows everything. From the moment it is made it is the global `fetch`, so npm packages such as Octokit, and the test itself, reach its fixtures and never the network; `host.dispose()` gives `fetch` back. Dispose mocks in the reverse order you made them (`t.after(() => host.dispose())` does).

`activate` may be `async`. Requests wait until its Promise settles, so what it registers after an `await` is there. If it rejects, the activation fails as in Codeyard: `await host.activate(extension)` rejects with `activate failed: …`, the log has the same line, and every request fails with it.

Each `MockHost` starts the SDK afresh, but your module is loaded once per test file, so its top-level variables (a cached client, a timer handle) carry over from one test to the next. Reset them in `activate`, or import the module afresh for a test: `await import("../src/extension.ts?case=2")`.

## In CI

`cyx new` writes a publish workflow; to also test on every push, add a job that runs the unit tests: `cargo test` for Rust (no WebAssembly target needed), or `npm ci && npm test` for TypeScript (Node.js 22.18 or newer). `cyx test` needs Codeyard's extension host, and for Rust the `wasm32-wasip2` target, so run it where Codeyard is installed, or install `cyx` and the extension host with the setup action, pinned to a commit like the publish action: `uses: doancan/codeyard/actions/setup-cyx@<commit>`.
