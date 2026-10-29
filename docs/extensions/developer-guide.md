# Writing Codeyard extensions

Themes, snippets, keymaps, languages, schemas and language servers without code; commands, views and providers in Rust with code extensions. If you have built a VS Code extension, most of this will feel familiar. Parts marked **(draft)** may still change before API 1.0, and parts marked **(coming)** are not in this version yet.

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

Commands, views, formatters and code actions need code: a Rust crate that `cyx` builds into a WebAssembly component. You need Rust and its WebAssembly target (`rustup target add wasm32-wasip2`). The Rust SDK is pre-release; TypeScript comes later. The full guide is [commands.md](commands.md).

```sh
cyx new command hello     # or: cyx new view hello
cd hello
cargo test                # unit tests against a mock Codeyard
cyx test                  # the built extension in the real extension host
```

```toml
[code]
entry = "Cargo.toml"

[[commands]]
id    = "hello"              # becomes you.hello.hello
title = "Hello: Say Hello"
```

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

Calls that reach Codeyard are `async`; the SDK runs them without blocking the extension. The build uses plain cargo with the `wasm32-wasip2` target, and `cyx package` does it for you. Running code extensions in the `cyx dev` window, with reload on save, arrives with the app side of code extensions **(coming)**.

You don't have to write any activation code. The extension starts the first time someone uses one of its commands, views or languages.

## The manifest

`codeyard-extension.toml`. Most extensions need fewer than 15 lines.

| Table / field | Required | What it does |
| --- | --- | --- |
| `extension.id` | yes | `<publisher>.<name>`, lowercase `a-z 0-9 -` |
| `extension.version`, `name`, `license` | yes | `description`, `repository`, `categories` and `icon` are optional |
| `code.entry` | for code | `Cargo.toml` means Rust, `.ts`/`.js` means TypeScript. Leave it out for themes and other no-code packs |
| `permissions` | if used | `read`, `write`, `net`, `run`, `auth`, `secrets`, `clipboard`, `terminal`, `commands`, plus `why` ([details](#permissions)) |
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
command  = "refresh"            # one of your commands, or a dependency's full id
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
```

A file-tree menu entry passes the file's URI to the command as its argument. A code action's edits are applied when
the user picks it, then its `command` runs; an action with neither gets `request.run-code-action`. Your extension
starts when a file of one of these languages opens or a provider is first asked. Providers have one second
(`PROVIDER_DEADLINE_MILLIS`) to answer, plus the start-up time on the first request; a later
answer is dropped and noted in your log, and a request about a document is cancelled as soon as the user types in it.
A task runs in your extension: start processes with `process.spawn` and show progress. Everything you showed goes when
the user disables or uninstalls the extension, or when it crashes.

## Permissions

An extension can only do what its manifest declares. If it tries anything else, the call is **denied** and the user is not asked. You don't have to guess which permissions you need, because `cyx` tells you:

```
error[CY0201]: net access to "api.vercel.com" is not declared
  --> codeyard-extension.toml
   = fix: add to [permissions]:  net = ["api.vercel.com"]
```

`cyx check --fix` adds the missing entries for you (draft).

```toml
[permissions]
read = ["vercel.json"]          # workspace globs; .env* and .git/ must be named explicitly
net  = ["api.vercel.com"]       # HTTPS to these hosts
run  = ["vercel"]               # these programs, any arguments
why  = "Shows your Vercel deployments next to the code."
```

**You don't need a permission for:** notices, quick picks, input boxes, progress, your own views, status items, storage and settings, `env`, your dependencies' commands, or opening an `https:` link after a user action. A command the user starts can read and edit the active document and selections until it returns.

**What users see.** When someone installs your extension they get one sheet that lists your permissions in plain words ("Connect to api.vercel.com", "Run vercel with any arguments") and quotes your `why`. Some combinations count as high risk: anything that *reads* private data (a wide `read`, files like `.env` or `.git/`, clipboard, secrets, an account) together with anything that *sends* (`net`, `run`, `write`, terminal), and anything that can run code as the user (`run`, terminal, or writing build files and hooks). That sheet gets a red banner, and for those combinations `why` is required. Write it in one honest sentence, 200 characters at most.

To see exactly what users will see:

```sh
cyx dev --as-user        # run with the real consent sheet
cyx publish --dry-run    # print the sheet without publishing
```

If an update **removes** a permission, it installs silently. If it **adds** one, users see a card listing only the new ones and keep the old version until they accept.

## Recipes

The recipes below show the planned TypeScript API **(coming)**; [commands.md](commands.md) shows the same things in Rust, and `examples/extensions/` has working Rust extensions. All API shapes are (draft).

### Status bar item

```toml
[[status-bar]]
id      = "deploy"
command = "refresh"
```

```ts
export function activate(ctx: Context) {
  const item = ctx.statusBar.get("deploy");
  item.set({ text: "Deploy: ready", tone: "success" });
  ctx.every(60, () => item.set({ text: "Deploy: checking…" }));   // minimum 30 s, pauses when unfocused
}
```

### Quick pick

```ts
ctx.commands.register("pick-env", async () => {
  const env = await ctx.ui.quickPick(["production", "preview", "development"], {
    placeholder: "Environment",
  });
  if (env) await ctx.storage.set("env", env);
});
```

### Settings

```toml
[settings.interval]                # shown under Settings › Extensions › Deploy Status (draft) shape
type        = "number"
default     = 60
description = "Seconds between refreshes."
```

```ts
const seconds = await ctx.settings.get<number>("interval");
ctx.settings.onChange("interval", (v) => restartTimer(v));
```

### Call an API with the user's GitHub login

The user signs in once, and Codeyard keeps the token. Your extension never sees it, and users never paste a token.

```toml
[permissions]
auth = ["github"]
net  = ["api.github.com"]
```

```ts
const res = await fetch("https://api.github.com/user/repos", { auth: "github" });   // (draft) option
const repos = await res.json();
```

You can also use another OAuth 2 provider. Declare its authorize and token URLs in the manifest (draft), and Codeyard runs the browser sign-in.

### Tree view

Views are built from built-in elements such as `tree`, `list`, `table`, `button`, `text` and `badge`. Codeyard draws them with the user's theme and screen-reader support. You don't choose colours or fonts, only tones (`accent`, `success`, `warning`, `danger`, `muted`). The guide is [Views](views.md); every element and event is in the [views reference](reference/ui.md).

```toml
[[views]]
id       = "deployments"
title    = "Deployments"
location = "sidebar"          # sidebar | bottom | editor
```

```ts
import { ui } from "@codeyard/extension";

ctx.views.register("deployments", async () =>
  ui.tree(deployments.map((d) => ({
    key: d.id,
    label: d.name,
    badge: { text: d.state, tone: d.state === "ready" ? "success" : "warning" },
    children: () => loadBuilds(d.id),          // loaded when expanded
  })))
);

// When data changes:
ui.invalidate("deployments");
```

The SDK turns this into the JSON tree Codeyard draws (`response.view`) and loads tree levels one page at a time. There are no webviews, so you can't build HTML previews or charts in v1.

### Diagnostics

```toml
[[diagnostics]]
id        = "todos"
languages = ["rust", "typescript"]
```

```ts
ctx.diagnostics.register("todos", async (doc) =>
  doc.lines()
    .map((text, line) => ({ line, col: text.indexOf("TODO") }))
    .filter((m) => m.col >= 0)
    .map((m) => ({ range: [m.line, m.col, m.line, m.col + 4], severity: "info", message: "TODO left here" }))
);
```

A provider receives its document, so it needs no `read` permission. Answer quickly, because the call is cancelled once the user moves on.

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
await ctx.ui.progress("Building…", async () => {
  for await (const line of ctx.spawn("vercel", ["build"])) ctx.log(line);   // needs run = ["vercel"]
});
```

## Test and debug

**Unit tests** run natively against a mock host with fake files, recorded processes, fixture HTTP and a virtual clock. The whole story is in [testing.md](testing.md).

```rust
#[test]
fn greets() {
    let host = MockHost::new();          // codeyard_extension_test::MockHost
    host.activate::<Hello>().unwrap();
    host.answer_input("Ada");
    host.command("hello", Value::Null).unwrap();
    assert_eq!(host.notices(), ["Hello, Ada!"]);
}
```

```sh
cargo test     # Rust unit tests
cyx test       # the real built extension in the extension host; fails on any refused call, with its fix
npm test       # TypeScript (coming)
```

**Debug.** Open the palette and run **Developer: Extension Inspector**. It shows:
- why and when your extension activated
- every call, with how long it took: for a command, the time in your code; for a call to Codeyard, the time the work
  took (I/O) and the time around it (overhead)
- every denied call, with a **Copy fix** button that copies the `[permissions]` line to add
- logs with their level, and the last crash with its frames (file:line as far as your build's debug info allows)

The live view tree with highlighting and source maps for TypeScript are not there yet; they come with extension views.

While a debugger is attached, nothing times out. `storage` keeps data across sessions, and in `cyx dev` hot reload also keeps your in-memory state.

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
- **Updates with code** reach users after **48 hours**, and `cyx publish` tells you when. Users can press **Update now**.
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
| `vscode.commands.registerCommand` | `ctx.commands.register` (draft) |
| `window.showQuickPick` / `showInputBox` | `ctx.ui.quickPick` / `ctx.ui.input` (draft) |
| `createStatusBarItem` | `[[status-bar]]` + `ctx.statusBar` (draft) |
| `TreeDataProvider` | `[[views]]` + `ui.tree` |
| Webview | Not available. Use built-in elements |
| `globalState` / `workspaceState` | `ctx.storage` |
| `SecretStorage` | `secrets` permission. For sign-in, prefer `auth` |
| `authentication.getSession` | `auth = ["github"]` |
| `DiagnosticCollection` | `[[diagnostics]]` provider |
| `fs`, `child_process` | `read`/`write` permissions, `ctx.spawn` |
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
| `Cannot find module 'fs'` (TypeScript) | Node built-ins are not available. Use `ctx` file APIs, or `fetch` for HTTP |
| Command missing from the palette | Check the id. Commands are shown as `Title`, but the full id is `you.ext.id` |
| Provider never shows results | It answered too late. Return quickly and move slow work into `progress` |
| Language server does not start | Add the program to `run`, trust the folder, and check that the program is on the workspace machine's PATH |
| `cyx: command not found` | Run **Extensions: Install cyx Command in PATH**. In CI, use `codeyard/setup-cyx` |
| Extension disabled after crashes | Open the Inspector for the stack, then fix and reload. Users can **Reinstall previous version** |
| My update hasn't reached users | Code updates wait 48 h. Use `--pre-release` for testers |
