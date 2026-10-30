# Commands, views and other code

A code extension is a Rust crate built into a WebAssembly component. It adds commands to the palette, views to the sidebar, formatters, code actions and anything else that needs code. This page covers writing one; [testing.md](testing.md) covers testing it, and [reference/api.md](reference/api.md) lists every call. For TypeScript, see [typescript.md](typescript.md): the same API, with Promises.

The SDK is **pre-release**: it is not on crates.io yet, so a new extension takes it from the Codeyard repository (see [The SDK dependency](#the-sdk-dependency)). Its API may still change before 1.0.

<!-- toc -->
- [Setup](#setup)
- [Your first command](#your-first-command)
- [How calls work](#how-calls-work)
- [Commands](#commands)
- [Views](#views)
- [Talking to the user](#talking-to-the-user)
- [Documents, diagnostics and decorations](#documents-diagnostics-and-decorations)
- [Formatters and code actions](#formatters-and-code-actions)
- [Hover and completion](#hover-and-completion)
- [Code lenses and inlay hints](#code-lenses-and-inlay-hints)
- [Network, files and programs](#network-files-and-programs)
- [Storage, settings and timers](#storage-settings-and-timers)
- [Errors and refused calls](#errors-and-refused-calls)
- [Building and packaging](#building-and-packaging)
- [The SDK dependency](#the-sdk-dependency)
<!-- /toc -->

## Setup

You need Rust (from [rustup.rs](https://rustup.rs)) and its WebAssembly target:

```sh
rustup target add wasm32-wasip2
```

No other tools: the build is plain `cargo`.

## Your first command

```sh
cyx new command hello      # or: cyx new view hello
cd hello
cargo test                 # unit tests, natively, against a mock Codeyard
cyx test                   # the built extension in the real extension host
cyx dev                    # Codeyard with the extension loaded
```

`cyx new command` writes a crate with one command. The manifest says where the code is and what the command is called:

```toml
[code]
entry = "Cargo.toml"

[[commands]]
id    = "hello"                 # becomes you.hello.hello
title = "Hello: Say Hello"
```

And `src/lib.rs` registers it:

```rust
use codeyard_extension::prelude::*;

struct Hello;

impl Extension for Hello {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("hello", async |ctx, _args| {
            let name = ctx.input(InputOptions::prompt("Your name?")).await?;
            let name = name.unwrap_or_else(|| "world".into());
            ctx.notice(format!("Hello, {name}!"));
            Ok(())
        });
        Ok(())
    }
}

codeyard_extension::export!(Hello);
```

`activate` runs once, the first time someone uses one of the extension's commands or views. It registers handlers and returns; it does not need to know why it started (`ctx.activation_reason()` says, if you want to).

The complete template, with its test, is [examples/extensions/hello-command](../../examples/extensions/hello-command). The view template is [examples/extensions/hello-view](../../examples/extensions/hello-view).

## How calls work

Anything that reaches Codeyard, such as a notice with buttons, a file read or an HTTPS request, is an `async` function: `ctx.fetch(…).await`. Handlers are async closures (`async |ctx, args| { … }`), so you can `.await` inside them. Calls that Codeyard answers on the spot, such as storage, settings, logging, diagnostics and status items, are plain functions.

Under the hood every async call returns a ticket at once, and the SDK resumes your code when Codeyard delivers the result. The extension never blocks while it waits, so it can answer other requests at the same time. You don't manage any of this; two rules are enough:

- **Don't block.** There are no threads and no `std::thread::sleep`. A handler that computes for more than 2 seconds without awaiting is stopped. Long work awaits calls, and shows progress.
- **Answer fast.** Providers (views, formatters, code actions, hover, completion) are cancelled when the user moves on. The SDK drops a cancelled handler, which cancels the calls it was waiting on.

`ctx.background(async move { … })` runs a task that no request waits for, such as work started from `activate`.

`Context` is `Copy`: capture it in closures freely. Keep state in `Rc<RefCell<…>>` or a `thread_local!`; an extension has one thread.

## Commands

```rust
ctx.command("refresh", async |ctx, args| {
    // `args` is the JSON the command was run with (`Value::Null` from the palette).
    let deployments = load(&ctx).await?;
    Ok(json!({ "count": deployments.len() }))     // anything Serialize; Ok(()) for nothing
});
```

The id is the local id from `[[commands]]`. Run other commands with `ctx.execute("other.ext.cmd", json!([…])).await`.
Codeyard's own commands start with `core:`. Only those in [reference/core-commands.md](reference/core-commands.md) can
be run, and each must be listed in `[permissions] commands`. Each also implies a permission you must hold: Save All
needs `write` on every file it saves, `core:editor.open` needs `read` on the file, and `core:tasks.run` needs the
task's program in `run`:

```rust
ctx.execute("core:editor.open", json!(format!("file://{root}/docs/guide.md"))).await?;
```

A command that is not in the list is refused with the fix, and so is one you have not listed or whose implied
permission you lack. `cyx check` warns about a `core:` entry in `permissions.commands` that is not in the list.

## Views

A view is a tree of built-in elements that Codeyard draws with the user's theme. Declare it, then return the tree as JSON:

```toml
[[views]]
id       = "main"
title    = "Clicks"
location = "sidebar"            # sidebar | bottom | editor
```

```rust
ctx.view("main", async |ctx| {
    Ok(json!({
        "type": "column",
        "children": [
            { "type": "text", "text": "3 clicks" },
            { "type": "button", "key": "add", "label": "Add one" }
        ]
    }))
});
ctx.on_view_event("main", async |ctx, event| {
    // A click, a changed input, a selection: `event` says which element.
    ctx.invalidate("main");     // draw the view again
    Ok(())
});
```

The element format is being finalised with the view renderer; until then the JSON shape above may change, and a typed builder will replace hand-written JSON. There are no webviews.

## Talking to the user

None of these need a permission. Every surface shows your publisher id.

```rust
ctx.notice("Deployed.");                                                     // does not wait
let choice = ctx.notify(Severity::Warning, "Deploy failed", &["Retry", "Logs"]).await?;   // Some(0), Some(1) or None
let env = ctx.quick_pick(["production", "preview"], PickOptions::default()).await?;      // Some(index) or None
let name = ctx.input(InputOptions::prompt("Branch?")).await?;               // Some(text) or None

let progress = ctx.progress("Building…", true);
progress.report(Some("step 2 of 3"), Some(66));
if progress.is_cancelled() { /* the user pressed Cancel */ }
drop(progress);                                                              // or progress.end()

ctx.set_status_item(StatusItem { id: "deploy".into(), text: "Deploy: ready".into(), tone: Tone::Success, ..Default::default() })?;
ctx.open_external("https://vercel.com/dashboard").await?;                    // only while handling a user action
ctx.log(LogLevel::Info, "refreshed");                                        // println! goes to the log too
```

## Documents, diagnostics and decorations

A command the user started may read and edit the active document and its selections until it returns, with no permission. Anything else needs `read` (and `write` for edits) on the document's path.

```rust
if let Some(doc) = ctx.active_document().await? {
    let edits = [TextEdit::replace(Range::line(0, 0, 0), "// header\n")];
    ctx.apply_edits(&doc.uri, doc.version, &edits).await?;      // fails if the document changed since
}
ctx.set_diagnostics(&uri, &[Diagnostic::new(Range::line(3, 0, 4), DiagnosticSeverity::Information, "TODO left here")]);
ctx.set_decorations(&uri, "coverage", &decorations);
```

Positions are zero-based lines and UTF-16 columns, as in the Language Server Protocol.

## Formatters and code actions

Providers receive their document, so they need no `read` permission.

```rust
ctx.formatter(async |ctx, doc, range, options| {
    Ok(vec![TextEdit::replace(whole(&doc), format(&doc.text, options.tab_size))])
});

ctx.code_actions(async |ctx, doc, range, diagnostics| {
    Ok(vec![CodeAction { id: "fix-todo".into(), title: "Remove TODO".into(), edits: vec![(doc.uri, edits)], ..Default::default() }])
});
ctx.on_code_action(async |ctx, id| Ok(()));   // for actions with a `command` instead of edits
```

## Hover and completion

Declare `[[hover]]` and `[[completion]]` with their languages (and, for completion, optional `trigger-characters`).
What you return shows next to the language server's, under your extension's id. Answer from the document you are
given, within the provider deadline (one second): a later answer is dropped and noted in your log.

```rust
ctx.hover(async |ctx, doc, position| {
    Ok(Some(Hover::new("**TODO**: work left to do.")))   // Markdown, shown as plain text; `None` for nothing
});

ctx.completion(async |ctx, doc, position, trigger| {
    // `trigger` is the trigger character typed, `None` when the user typed a word or asked.
    Ok(vec![CompletionItem { insert_text: Some("TODO: $0".into()), snippet: true, ..CompletionItem::new("TODO", CompletionKind::Keyword) }])
});
```

A completion answer is a `Vec<CompletionItem>`, or a `CompletionList` with `incomplete: true` when more typing changes
it (you are asked again on each key instead of the list only being filtered). An item may set its `range`,
`filter_text`, `sort_text`, `documentation` and `additional_edits` (an import, say).

## Code lenses and inlay hints

A code lens is clickable text shown with its line; a click runs a command with the arguments you gave. An inlay hint is
short text at the end of its line, such as a type. Declare `[[code-lens]]` and `[[inlay-hints]]` with their languages
(see the [developer guide](developer-guide.md#contributions-that-run-your-code)). Codeyard asks only for the lines in view: `range`
starts at the first one and ends at the start of the line after the last. It asks again once typing pauses after an
edit and once the view stops after a scroll, and cancels the question before; typing never waits for the answer.

```rust
ctx.code_lenses(async |ctx, doc, range| {
    // Runs `run-test` (one of your [[commands]]) with { "name": "parses" } when clicked.
    Ok(vec![CodeLens::new(12, "Run test", "run-test", json!({ "name": "parses" }))])
});
ctx.inlay_hints(async |ctx, doc, range| Ok(vec![InlayHint::new(Position::new(3, 18), ": u32")]));
```

A lens's command is one of your commands (local or full id), a dependency's full id, or a `core:` command your
manifest lists in `permissions.commands`; anything else is refused when clicked. When working out a lens is costly,
return `CodeLens::unresolved(line, id)` and fill it in when it is on screen:

```rust
ctx.resolve_code_lens(async |ctx, doc, mut lens| {
    lens.command = Some(LensCommand { title: "3 references".into(), command: "refs".into(), args: json!(lens.id) });
    Ok(lens)
});
```

## Network, files and programs

These need permissions in the manifest, and Codeyard checks every call against them.

```toml
[permissions]
net  = ["api.vercel.com"]
read = ["vercel.json"]
run  = ["vercel"]
```

```rust
let res = ctx.fetch(Request::get("https://api.vercel.com/v6/deployments").header("accept", "application/json")).await?;
let body: Value = res.error_for_status()?.json()?;

let config = ctx.read_text(&format!("{}/vercel.json", ctx.folders()[0])).await?;
let found = ctx.find_files("**/vercel.json", 10).await?;

// The whole output at once…
let out = ctx.run(Command::new("vercel").args(["ls", "--json"])).await?;
// …or as it comes.
let mut build = ctx.spawn(Command::new("vercel").arg("build"));
while let Some(chunk) = build.next().await {
    if let Chunk::Stdout(bytes) = chunk { ctx.log(LogLevel::Info, String::from_utf8_lossy(&bytes)); }
}
let code = build.wait().await?;            // dropping `build` before it ends kills the process
```

Requests leave from the machine Codeyard runs on, and programs run where the workspace is (so also over SSH). `ctx.fetch_as("github", request)` signs a request with the user's account (`auth = ["github"]`); the extension never sees the token. `ctx.secret`, `ctx.read_clipboard` and `ctx.send_to_terminal` need `secrets`, `clipboard` and `terminal`.

## Storage, settings and timers

```rust
let n = ctx.storage().get_json::<u32>("runs")?.unwrap_or(0);   // persisted, 50 MiB in all
ctx.storage().set_json("runs", &(n + 1))?;

let seconds = ctx.setting::<u32>("interval")?.unwrap_or(60);   // declared under [settings.interval]
ctx.on_settings_changed(async |ctx, keys| { /* restart the timer */ });

let timer = ctx.every(seconds, async |ctx| { /* poll */ });      // at least 30 s, paused while unfocused
timer.clear();

let clicks: u32 = ctx.state().get("clicks").unwrap_or(0);       // in memory; kept across a hot reload in cyx dev
ctx.state().set("clicks", &(clicks + 1))?;
```

`ctx.state()` holds JSON values in memory. When `cyx dev` swaps in a new build, the SDK saves them as the old build stops and the new one gets them back (it starts with `ctx.activation_reason() == "reload"`); any other start begins empty. They pass through storage under the reserved key `codeyard.reload-state`.

## Errors and refused calls

Every call returns `codeyard_extension::Result`. `?` passes errors up, and a handler that returns `Err` answers Codeyard with that error. Strings and `serde_json` errors convert with `?` too; `Error::other(…)` makes one from any message. `Error::Denied` means Codeyard refused the call, and printing it says what to change:

```
denied: the manifest does not declare net = ["api.example.com"]
   = fix: add to [permissions]:  net = ["api.example.com"]
```

Other errors are `NotFound`, `Cancelled` (the user moved on), `Timeout`, `Invalid` (bad arguments; the message says which) and `Other`. A refusal for `revoked-by-user` means the user turned the permission off: tell them what stops working. In an untrusted folder an extension may read what it declared and show UI, but not send, write or run anything.

## Building and packaging

```sh
cyx package        # cargo build --release --target wasm32-wasip2, then extension.wasm, the checks and the .cyx
```

`cyx package` builds the crate, copies the component to `extension.wasm` in the extension's folder, checks that its imports match the manifest (a component that can call `net` needs `net` declared, and so on) and writes the package. `cyx test` and `cyx dev` build the same way. `cyx check` checks the `extension.wasm` that is there.

`cyx package --no-build` skips the build and packages the `extension.wasm` already in the folder, for a component
built by another tool.

`cyx dev` builds once when it starts. Rebuilding and reloading on save, with in-memory state kept, comes with the dev window's support for code extensions.

## The SDK dependency

`cyx new command` writes:

```toml
[dependencies]
codeyard-extension = { git = "https://github.com/doancan/codeyard" }

[dev-dependencies]
codeyard-extension-test = { git = "https://github.com/doancan/codeyard" }
```

To work against a local Codeyard checkout instead, pass `cyx new command hello --sdk ../codeyard`, or change the lines to `{ path = "../codeyard/crates/ext-sdk" }` and `{ path = "../codeyard/crates/ext-test" }`. Once the SDK is on crates.io the template will use a version.
