# Writing Codeyard extensions

Themes, snippets, keymaps, languages, schemas and language servers without code; commands, views and providers in TypeScript or Rust with code extensions. If you have built a VS Code extension, most of this will feel familiar. Parts marked **(draft)** may still change before API 1.0, and parts marked **(coming)** are not in this version yet.

<!-- toc -->
- [Quick start](#quick-start)
- [A theme or snippet pack](#a-theme-or-snippet-pack)
- [Your first command](#your-first-command)
- [The manifest](#the-manifest)
- [Permissions](#permissions)
- [Recipes](#recipes)
- [Test and debug](#test-and-debug)
- [Publish](#publish)
- [Coming from VS Code](#coming-from-vs-code)
- [Troubleshooting](#troubleshooting)
<!-- /toc -->

## Quick start

1. **Get `cyx`.** Open the palette and run **Extensions: Install cyx Command in PATH**. It ships with Codeyard, so there is nothing to download.
2. **Scaffold.**
   ```sh
   cyx new theme my-theme       # or: snippets, keymap, language, language-server
   cd my-theme
   ```
   Or, without a terminal: **Extensions: New Extension…** in the palette asks for the kind, the name and a folder, then loads the new extension in this window, ready to edit.
3. **Run.**
   ```sh
   cyx dev
   ```
   Codeyard opens with your extension loaded and a **DEV** badge in the status bar. Save a file and it reloads within a second; a mistake shows up as a message with the fix instead.
4. **Check and package.**
   ```sh
   cyx check                    # every problem, with its file, line and fix
   cyx package                  # writes you.my-theme-0.1.0.cyx
   ```
   Install the `.cyx` from the Extensions view, or send it to someone.

That's all. The only file you have to have is `codeyard-extension.toml`.

## A theme or snippet pack

You don't need code or a toolchain for this. Codeyard is all you need.

```sh
cyx new theme midnight        # or: cyx new snippets my-snippets
cd midnight && cyx dev
```

```toml
# codeyard-extension.toml
[extension]
id      = "you.midnight"
version = "0.1.0"
name    = "Midnight"
license = "MIT"

[[themes]]
label = "Midnight"
path  = "themes/midnight.json"   # Codeyard or VS Code colour theme JSON

[[snippets]]
language = "rust"
path     = "snippets/rust.json"  # VS Code snippet format
```

Edit `themes/midnight.json`. The dev window updates as soon as you save. Codeyard checks text contrast and warns you if a colour pair is hard to read.

Do you already have a VS Code theme, snippets, keymap or language configuration?

```sh
cyx import-vscode ./my-vscode-theme     # a folder or a .vsix
```

It converts everything it can and lists anything it could not convert.

The same goes for other no-code contributions: `keymap-preset`, `keybindings`, `languages`, `json-schemas`, `yaml-schemas`, `settings` and `language-servers`. A few rules keep them predictable:

- A key binding never takes a key a built-in shortcut uses; Codeyard leaves it out and says so. A keymap preset may, because the user picks it (`"keymap.preset": "you.emacs/Emacs"`).
- A language you add applies to files no built-in language claims. It can borrow a built-in grammar for highlighting (`grammar = "toml"`).
- A language server is a program on `PATH` where the files are, so it works over SSH too.
- Start Codeyard with `--no-extensions` to rule your extension out when something looks wrong.

## Your first command

Commands, views, formatters, code actions, hover and completion need code, which `cyx` builds into a WebAssembly component. Write it in TypeScript (you need Node.js 22.18 or newer) or in Rust (you need Rust and `rustup target add wasm32-wasip2`). Both SDKs are pre-release and have the same API; the full guides are [typescript.md](typescript.md) and [commands.md](commands.md).

```sh
cyx new command hello --lang ts   # or: cyx new view hello --lang ts; leave out --lang ts for Rust
cd hello
npm install && npm test           # unit tests against a mock Codeyard (Rust: cargo test)
cyx test                          # the built extension in the real extension host
```

```toml
[code]
entry = "src/extension.ts"      # Rust: "Cargo.toml"

[[commands]]
id    = "hello"                 # becomes you.hello.hello
title = "Hello: Say Hello"
```

```ts
// src/extension.ts
import type { Context } from "@codeyard/extension";

export function activate(ctx: Context) {
  ctx.command("hello", async (ctx) => {
    const name = await ctx.input({ prompt: "Your name?" });
    if (name) ctx.notice(`Hello, ${name}!`);
  });
}
```

The same in Rust:

```rust
// src/lib.rs
use codeyard_extension::prelude::*;

struct Hello;

impl Extension for Hello {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("hello", async |ctx, _args| {
            if let Some(name) = ctx.input(InputOptions::prompt("Your name?")).await? {
                ctx.notice(format!("Hello, {name}!"));
            }
            Ok(())
        });
        Ok(())
    }
}

codeyard_extension::export!(Hello);
```

Calls that reach Codeyard return Promises (Rust: they are `async`); the SDK runs them without blocking the extension. `cyx package` builds the component for you: esbuild and ComponentizeJS for TypeScript (installed by `cyx` itself), plain cargo for Rust.

`cyx dev` runs it in Codeyard and **hot-reloads** it: when you save a source file, `cyx` rebuilds `extension.wasm` and the dev window swaps the new build in. The old build's `deactivate` runs, the new one is compiled (only the functions that changed, with Cranelift's incremental cache), activated with the reason `reload`, and open views are drawn again. Calls made meanwhile wait for the new build. A build that fails prints its errors in the terminal and the last good build keeps running.

Values you keep in `ctx.state` survive a reload; everything else in memory starts fresh. Use `storage` for what must outlast the session.

```rust
let n: u32 = ctx.state().get("clicks").unwrap_or(0) + 1;   // Rust
ctx.state().set("clicks", &n)?;
```

```ts
const n = (ctx.state.get<number>("clicks") ?? 0) + 1;      // TypeScript
ctx.state.set("clicks", n);
```

The SDKs save `ctx.state` as JSON in your extension's storage under the reserved key `codeyard.reload-state` when the old build stops, and read it back (and delete it) when the new one starts with the reason `reload`. Any other start begins with an empty `ctx.state`. Don't use that key yourself.

You don't have to write any activation code. The extension starts the first time someone uses one of its commands, views or languages.

## The manifest

`codeyard-extension.toml`. Most extensions need fewer than 15 lines.

| Table / field | Required | What it does |
| --- | --- | --- |
| `extension.id` | yes | `<publisher>.<name>`, lowercase `a-z 0-9 -` |
| `extension.version`, `name`, `license` | yes | `description`, `repository`, `categories` and `icon` are optional |
| `code.entry` | for code | `Cargo.toml` means Rust, `.ts`/`.js` means TypeScript. Leave it out for themes and other no-code packs |
| `permissions` | if used | `read`, `write`, `net`, `run`, `auth`, `secrets`, `clipboard`, `terminal`, `commands`, plus `why` ([details](#permissions)) |
| `[[oauth]]` | for an OAuth account | An OAuth 2 provider named in `auth` ([details](#call-an-api-with-the-users-github-login)) |
| `activation` | rarely | Codeyard works it out from your contributions. The only form you ever write is `workspace-contains = ["vercel.json"]` |
| `[[commands]]`, `[[views]]`, `[[status-bar]]`, … | — | Contributions. Ids are local: `refresh` becomes `you.ext.refresh` |
| `when`, `enablement` | — | `editor.language == 'rust' && !terminal`. Keys: `editor`, `terminal`, `file-tree`, `scm`, `search`, `panel`, `workspace.trusted`, `editor.language`, `ext.<key>` |
| `dependencies` | — | `{ "other.ext" = "1.2" }` is a minimum version. You can call that extension's commands by their full id. Core commands start with `core:` |
| `untrusted` | — | `false` keeps your extension off in untrusted folders. By default it runs there, but it cannot send, write or run anything |
| `l10n/<code>.tsv` | — | `English<TAB>Translation` per line. English is the fallback |

Never write an engine version, an API version, `*` or `startup`. `cyx` works out the minimum Codeyard version from what you use.

### Contributions that run your code

```toml
[[menus]]
command  = "refresh"            # one of your commands, a dependency's full id, or a core command
location = "editor-context"     # editor-context | file-tree | view-title (with view = "<your view>")
when     = "editor.language == 'toml'"   # hides the entry; a command's `enablement` greys it out

[[status-bar]]
id      = "deploy"              # set its text at run time with this id; an undeclared id is not shown
command = "refresh"             # run on click, unless the item names another

[[formatters]]
languages = ["toml"]            # request.format; `editor.formatters` lets users choose per language

[[code-actions]]
languages = ["toml"]            # request.code-actions; shown in Quick Fix next to the language server's

[[diagnostics]]
id        = "todos"
languages = ["rust"]            # you push them with languages.set-diagnostics

[[decorations]]
id        = "coverage"
languages = ["rust"]            # languages.set-decorations: gutter, line or after, in tones

[[tasks]]
id = "deployments"              # request.provide-tasks lists them, request.run-task runs one

[[hover]]
languages = ["toml"]            # request.hover; shown in the hover popup next to the language server's

[[completion]]
languages = ["toml"]            # request.completion; items join the language server's list
trigger-characters = ["."]      # optional: these characters ask too, besides typing a word
```

A file-tree menu entry passes the file's URI to the command as its argument, and a view-title entry the view's full id
(`you.ext.deployments`): view-title entries appear under the **…** button in that view's title bar. A menu entry may
also name a core command that takes no argument, such as `core:workbench.saveAll`
([the list](reference/core-commands.md)); the user's click runs it, so it needs no permission. A code action's edits are applied when
the user picks it, then its `command` runs; an action with neither gets `request.run-code-action`. Your extension
starts when a file of one of these languages opens or a provider is first asked. Providers have one second
(`PROVIDER_DEADLINE_MILLIS`) to answer, plus the start-up time on the first request; a later
answer is dropped and noted in your log, and a request about a document is cancelled as soon as the user types in it.
A completion request lasts while the user types the same word (your items are filtered as the word goes on) and is
cancelled when the word is left or the list closed; a hover request is cancelled when the mouse moves to another word.
Your hover text and items are shown under your extension's id. Over SSH you get the document as the editor has it.
A task runs in your extension: start processes with `process.spawn` and show progress. Everything you showed goes when
the user disables or uninstalls the extension, or when it crashes.

## Permissions

An extension can only do what its manifest declares. If it tries anything else, the call is **denied** and the user is not asked. You don't have to guess which permissions you need, because `cyx` tells you:

```
error[CY0201]: net access to "api.vercel.com" is not declared
  --> codeyard-extension.toml
   = fix: add to [permissions]:  net = ["api.vercel.com"]
```

`cyx check --fix` adds the missing entries for you. It edits `codeyard-extension.toml` in place (comments, order and spacing stay as you wrote them), prints each line it added or changed, and lists the fixes it cannot make for you: a placeholder such as `net = ["<host>"]` from the component's imports, which provider an `auth` is for, and `why`.

```
$ cyx check --fix
Fixed codeyard-extension.toml [permissions]:
  run = ["git", "taplo"]
Not fixed, these need you:
  net = ["<host>"]   (put in the real value)
```

```toml
[permissions]
read = ["vercel.json"]          # workspace globs; .env* and .git/ must be named explicitly
net  = ["api.vercel.com"]       # HTTPS to these hosts
run  = ["vercel"]               # these programs, any arguments
commands = ["core:workbench.saveAll"]   # core commands your code runs (reference/core-commands.md)
why  = "Shows your Vercel deployments next to the code."
```

**You don't need a permission for:** notices, quick picks, input boxes, progress, your own views, status items, storage and settings, `env`, your dependencies' commands, or opening an `https:` link after a user action. A command the user starts can read and edit the active document and selections until it returns.

**What users see.** When someone installs your extension they get one sheet that lists your permissions in plain words ("Connect to api.vercel.com", "Run vercel with any arguments") and quotes your `why`. Some combinations count as high risk: anything that *reads* private data (a wide `read`, files like `.env` or `.git/`, clipboard, secrets, an account) together with anything that *sends* (`net`, `run`, `write`, terminal), and anything that can run code as the user (`run`, terminal, or writing build files and hooks). That sheet gets a red banner, and for those combinations `why` is required. Write it in one honest sentence, 200 characters at most.

At run time, undeclared access is refused, never asked. Codeyard asks the user only on the first account use, the first clipboard read per session and the first program run on a remote (SSH) host per session; if they deny it, that call and the later ones of its kind fail with `revoked-by-user` until Codeyard quits, so handle that error and say what stops working.

To see exactly what users will see:

```sh
cyx dev --as-user        # run with the real consent sheet
cyx publish --dry-run    # print the sheet without publishing
```

If an update **removes** a permission, it installs silently. If it **adds** one, users see a card listing only the new ones and keep the old version until they accept.

## Recipes

The recipes use the TypeScript SDK; [commands.md](commands.md) shows the same things in Rust, and `examples/extensions/` has working extensions in both. `ctx` is the `Context` that `activate` receives.

### Status bar item

```toml
[[status-bar]]
id      = "deploy"
command = "refresh"
```

```ts
export function activate(ctx: Context) {
  ctx.setStatusItem({ id: "deploy", text: "Deploy: ready", tone: "success" });
  ctx.every(60, async (ctx) => {                                    // minimum 30 s, pauses when unfocused
    ctx.setStatusItem({ id: "deploy", text: "Deploy: checking…" });
  });
}
```

### Quick pick

```ts
ctx.command("pick-env", async (ctx) => {
  const envs = ["production", "preview", "development"];
  const i = await ctx.quickPick(envs, { placeholder: "Environment" });   // the index, or undefined
  if (i !== undefined) ctx.storage.setJson("env", envs[i]);
});
```

### Settings

```toml
[settings.interval]                # shown under Settings › Extensions › Deploy Status
type        = "number"
default     = 60
description = "Seconds between refreshes."
```

```ts
const seconds = ctx.setting<number>("interval") ?? 60;
ctx.onSettingsChanged(async (ctx, keys) => {
  if (keys.includes("interval")) restartTimer(ctx.setting<number>("interval") ?? 60);
});
```

### Call an API with the user's GitHub login

The user signs in once, and Codeyard keeps the token. Your extension never sees it, and users never paste a token.

```toml
[permissions]
auth = ["github"]
why  = "Lists your GitHub repositories."
```

```ts
const res = await fetch("https://api.github.com/user/repos", { auth: "github" });
const repos = await res.json();
```

The first `auth.session("github")` asks the user once, naming your extension, its publisher and the account. GitHub uses the sign-in Codeyard already has (the GitHub CLI). `auth.fetch` signs requests to `api.github.com` and `uploads.github.com` only. A request to any other host is refused and is not sent. Don't set an `authorization` header yourself. npm clients work too: `new Octokit({ request: { fetch: ctx.authFetch("github") } })` ([examples/extensions/octokit-repos-ts](../../examples/extensions/octokit-repos-ts)).

You can also use another OAuth 2 provider. Declare it in an `[[oauth]]` table and name it in `auth`. Codeyard runs the browser sign-in with PKCE and keeps the token. It refreshes the token when it is about to expire.

```toml
[permissions]
auth = ["acme"]

[[oauth]]
id            = "acme"
name          = "Acme"                                   # shown to the user
authorize-url = "https://acme.example.com/oauth/authorize"
token-url     = "https://acme.example.com/oauth/token"   # https only
client-id     = "your-public-client-id"                  # no client secret: PKCE replaces it
scopes        = ["read", "deploy"]                       # auth.session may ask for fewer
hosts         = ["api.acme.example.com"]                 # the hosts auth.fetch signs requests to
account-url   = "https://api.acme.example.com/me"        # optional: JSON with the account's name
```

Register `codeyard://ext/<your extension id>/callback` as the redirect URI with the provider. The user can sign out on your extension's page. The next `auth.fetch` then fails with "not signed in", and your extension calls `auth.session` again.

**Secrets.** If a service has no OAuth, `secrets = true` gives your extension its own keychain entries. Use `secrets.set(key, value)`, `secrets.get(key)` and `secrets.delete(key)`. Keys are `a-z A-Z 0-9 . _ -`, up to 128 characters. A value is at most 2048 bytes. The entries belong to your publisher identity, so a sideloaded copy of your extension cannot read them.

### Tree view

Views are built from built-in elements such as `tree`, `list`, `table`, `button`, `text` and `badge`. Codeyard draws them with the user's theme and screen-reader support. You don't choose colours or fonts, only tones (`accent`, `success`, `warning`, `danger`, `muted`). The guide is [Views](views.md); every element and event is in the [views reference](reference/ui.md).

```toml
[[views]]
id       = "deployments"
title    = "Deployments"
location = "sidebar"          # sidebar | bottom | editor
```

```ts
ctx.view("deployments", async (ctx) => ({
  type: "list",
  children: deployments.map((d) => ({
    type: "text",
    key: d.id,
    text: d.name,
    badge: { text: d.state, tone: d.state === "ready" ? "success" : "warning" },
  })),
}));

// When data changes:
ctx.invalidate("deployments");
```

The SDK turns this into the JSON tree Codeyard draws (`response.view`) and loads tree levels one page at a time. There are no webviews, so you can't build HTML previews or charts in v1.

### Diagnostics

```ts
ctx.command("find-todos", async (ctx) => {
  const doc = await ctx.activeDocument();          // a command the user started may read it without `read`
  if (!doc) return;
  const diagnostics = doc.text.split("\n").flatMap((text, line) => {
    const col = text.indexOf("TODO");
    if (col < 0) return [];
    const range = { start: { line, character: col }, end: { line, character: col + 4 } };
    return [{ range, severity: "information" as const, message: "TODO left here" }];
  });
  ctx.setDiagnostics(doc.uri, diagnostics);
});
```

A `[[diagnostics]]` provider that Codeyard calls on its own, as you type, is **(coming)**.

### A language server

You don't need any code for this.

```toml
[permissions]
run = ["taplo"]
why = "Starts the taplo language server for TOML files."

[[language-servers]]
command   = "taplo"
args      = ["lsp", "stdio"]
languages = ["toml"]
```

The server runs on the workspace machine, so it also works over SSH. It only starts in trusted folders.

### Long work and processes

```ts
await ctx.withProgress("Building…", async () => {
  const build = ctx.spawn({ program: "vercel", args: ["build"] });        // needs run = ["vercel"]
  for await (const chunk of build) if ("stdout" in chunk) ctx.log("info", new TextDecoder().decode(chunk.stdout));
  await build.wait();
});
```

### Native programs

Your extension can bring its own program, such as a language server binary or a CLI, instead of asking users to
install one. Name it in `run` as a path inside the package, starting with `./`:

```toml
[permissions]
run = ["./bin/hello"]
why = "Runs its bundled helper to read your project."
```

Build the program for each platform you support and put each build under `native/<triple>/`, at the path `run`
names:

```
native/aarch64-apple-darwin/bin/hello
native/x86_64-apple-darwin/bin/hello
native/x86_64-unknown-linux-musl/bin/hello     # static (musl), so one build runs on every distribution
native/aarch64-unknown-linux-musl/bin/hello
native/x86_64-pc-windows-msvc/bin/hello.exe    # Windows builds end in .exe
```

Such an extension is packaged once per platform. `cyx package` writes `<id>-<version>-<triple>.cyx` for every folder
in `native/` (`--target <triple>` writes one), and `cyx check` checks each. A per-platform package may be up to
128 MiB; it holds everything else of the extension too, and a `TARGET` file naming its platform. Programs must be
executable (`chmod 755`) except on Windows.

Your code starts the program like any other, with its `run` entry: `ctx.spawn({ program: "./bin/hello", args })`.
It runs on the computer that holds the workspace: this one, or the SSH server. Codeyard copies the program from the
package for that computer's platform, checks its SHA-256 there against the package's hash list before every run, and
runs only that copy; the server downloads nothing. A platform you did not build for gets an error that names it.

Users type your extension's id on the install sheet before a native program installs. Registries take native programs
from invited publishers only, and your first one waits for a human review before it is listed (see [Publish](#publish)).
Language servers (`[[language-servers]]`) still start only programs found on PATH.

## Test and debug

**Unit tests** run on your computer against a mock host with fake files, recorded processes, fixture HTTP and a virtual clock. The whole story is in [testing.md](testing.md).

```ts
test("greets", async () => {
  const host = new MockHost();           // from @codeyard/extension/testing
  await host.activate(extension);
  host.answerInput("Ada");
  await host.command("hello");
  assert.deepEqual(host.notices(), ["Hello, Ada!"]);
});
```

```sh
npm test       # TypeScript unit tests
cargo test     # Rust unit tests (codeyard_extension_test::MockHost)
cyx test       # the real built extension in the extension host; fails on any refused call, with its fix
```

**Debug.** Open the palette and run **Developer: Extension Inspector**. It shows:
- why and when your extension activated
- every call, with how long it took: for a command, the time in your code; for a call to Codeyard, the time the work
  took (I/O) and the time around it (overhead)
- every denied call, with a **Copy fix** button that copies the `[permissions]` line to add
- logs with their level, and the last crash with its frames (file:line as far as your build's debug info allows)

The live view tree with highlighting and source maps for TypeScript are not there yet; they come with extension views.

While a debugger is attached, nothing times out. `storage` keeps data across sessions, and in `cyx dev` hot reload also keeps what you put in `ctx.state` (see [Your first command](#your-first-command)).

## Publish

```sh
cyx publish --dry-run   # check everything and show the install sheet; nothing is signed or sent
cyx publish
```

There are no keys or tokens to create. The first `cyx publish` on your computer opens a browser to sign in with GitHub, and the release is signed for that one upload. `cyx new` already wrote `.github/workflows/publish.yml`, so you can also release by pushing a tag; the workflow signs with its own identity:

```sh
git tag v0.2.0 && git push --tags
```

The first publish claims your publisher name (the part of the id before the dot) for your GitHub account: any of your repositories can publish under it, nobody else can. After that, an extension is always published from the same repository and workflow it was first published from, so a stolen or forked copy cannot publish an update. Names that look like someone else's (`rnicrosoft`, `micros0ft`) are refused.

Good to know:
- **Themes, snippets and other no-code packs** reach users immediately.
- **Updates with code** reach users after **48 hours**, and `cyx publish` tells you when. Users can press **Update now**. The registry reads whether a package has code from the package itself and marks it in its index.
- **Code extensions on the public registry** are not accepted yet: for now it takes data-only extensions (themes, snippets, keymaps, languages, language servers). `cyx publish` says so when the registry refuses; share the `.cyx` from `cyx package` meanwhile, which installs from a file.
- **Native programs** ([Native programs](#native-programs)) are accepted by invitation: the registry's operator lists your publisher name. `cyx publish` signs and uploads one package per platform in `native/` (`--target <triple>` publishes one; you can add a platform to a version later, with the same manifest and README). Your first native program stays unlisted until a reviewer approves it; `cyx publish` says so. After that your native programs are listed like any code update.
- **Pre-releases** are versions with a suffix (`1.2.0-beta.1`). They go out at once, only to users who opted in for your extension. `cyx publish --pre-release` makes sure the version has one.
- If a release is broken, `cyx yank 1.2.0` hides it from installs and users on it roll back right away; `cyx restore 1.2.0` brings it back. `cyx deprecate 1.2.0 --replacement you.other` points users elsewhere. Version numbers are never reused, so publish the fix as `1.2.1`.
- `--registry URL` (or `CODEYARD_REGISTRY`) publishes to another registry, such as a local one from `yard init`.

## Coming from VS Code

| VS Code | Codeyard |
| --- | --- |
| `package.json` `contributes` | `codeyard-extension.toml` |
| `activationEvents` | Inferred. Use `workspace-contains` if needed. There is no `*` |
| `engines.vscode` | Not needed |
| `yo code` / `vsce package` | `cyx new` / `cyx package` |
| `vsce publish` + PAT | `cyx publish` or a tag push. No token |
| `vsce unpublish` | `cyx yank VERSION` (users roll back; the number stays used) |
| F5 Extension Development Host | `cyx dev` |
| `vscode.commands.registerCommand` | `ctx.command` |
| `window.showQuickPick` / `showInputBox` | `ctx.quickPick` / `ctx.input` |
| `createStatusBarItem` | `[[status-bar]]` + `ctx.setStatusItem` |
| `TreeDataProvider` | `[[views]]` + `ctx.view` |
| Webview | Not available. Use built-in elements |
| `globalState` / `workspaceState` | `ctx.storage` |
| `SecretStorage` | `secrets` permission. For sign-in, prefer `auth` |
| `authentication.getSession` | `auth = ["github"]` |
| `DiagnosticCollection` | `[[diagnostics]]` provider |
| `fs`, `child_process` | `read`/`write` permissions with `ctx.readFile` / `ctx.writeFile`, `run` with `ctx.spawn` |
| `extensionKind` | Not needed. The same extension works locally and over SSH |
| Colour themes, snippets, language config | Work unchanged, or use `cyx import-vscode` |
| Hover, completion, code lens, inlay hints | Coming with API 1.0 |

## Troubleshooting

Every error message shows a fix line. `cyx explain <CODE>` prints the full explanation, and `cyx doctor` checks your setup.

| You see | Fix |
| --- | --- |
| `error[CY0201]: net access to "…" is not declared` | Add the host to `[permissions] net`, or run `cyx check --fix` |
| `denied … not-declared` in the Inspector | Add the permission shown. Click **Copy fix** |
| `denied … untrusted-workspace` | Trust the folder, or set `untrusted = false` if the extension is useless without trust |
| `denied … revoked-by-user` | The user turned this permission off on the extension page. Show a helpful message |
| `why is required` warning on first build | Your permissions are high risk. Add one honest `why` sentence |
| `"fs" is a Node.js built-in module` (TypeScript build) | Node built-ins are not available. Use `ctx` file APIs, or `fetch` for HTTP, or a package that works in browsers |
| Command missing from the palette | Check the id. Commands are shown as `Title`, but the full id is `you.ext.id` |
| Provider never shows results | It answered too late. Return quickly and move slow work into `progress` |
| Language server does not start | Add the program to `run`, trust the folder, and check that the program is on the workspace machine's PATH |
| `cyx: command not found` | Run **Extensions: Install cyx Command in PATH**. In CI, use `codeyard/setup-cyx` |
| Extension disabled after crashes | Open the Inspector for the stack, then fix and reload. Users can **Reinstall previous version** |
| My update hasn't reached users | Code updates wait 48 h. Use `--pre-release` for testers |
