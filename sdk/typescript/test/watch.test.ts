// API 1.1 against the mock host: document and file watches (read and glob filtering, stop, refusal), openInEditor's
// argument, and fetches to a host the user allows for `net-from-setting`.

import assert from "node:assert/strict";
import { test } from "node:test";

import { DeniedError, type Context, type DocumentEvent, type FileEvent } from "../src/index.ts";
import { start } from "../src/guest.ts";
import { MockHost } from "../src/testing/index.ts";
import type { Bindings } from "../src/wit.ts";

const manifest = (permissions: string) => `
[extension]
id      = "you.todo"
version = "0.1.0"
name    = "TODO"
license = "MIT"

[code]
entry = "src/extension.ts"

[permissions]
${permissions}

[settings.server]
type    = "string"
default = "ghe.example.com"
`;

test("a document watch gets readable documents: those open first, then opened, changed and saved", async () => {
  const h = new MockHost({ manifest: manifest(`read = ["src/**"]\nwhy = "TODOs."`) });
  await h.openDocument("src/a.ts", "typescript", "// TODO one");
  await h.openDocument("notes.md", "markdown", "secret");
  const seen: DocumentEvent[] = [];
  await h.activate({
    activate(ctx: Context) {
      ctx.onDocuments((_ctx, e) => void seen.push(e), { text: true });
    },
  });
  await h.editDocument("src/a.ts", "// TODO two");
  await h.saveDocument("src/a.ts");
  await h.editDocument("notes.md", "still secret");
  const uri = h.uri("src/a.ts");
  assert.deepEqual(seen, [
    { change: "opened", uri, language: "typescript", version: 1, text: "// TODO one" },
    { change: "changed", uri, language: "typescript", version: 2, text: "// TODO two" },
    { change: "saved", uri, language: "typescript", version: 2, text: "// TODO two" },
  ]);
  h.dispose();
});

test("without text, events carry the version only", async () => {
  const h = new MockHost({ manifest: manifest(`read = ["*.ts"]`) });
  const seen: DocumentEvent[] = [];
  await h.activate({
    activate(ctx: Context) {
      ctx.onDocuments((_ctx, e) => void seen.push(e));
    },
  });
  await h.openDocument("a.ts", "typescript", "x");
  assert.deepEqual(seen, [{ change: "opened", uri: h.uri("a.ts"), language: "typescript", version: 1 }]);
  h.dispose();
});

test("a file watch gets its glob's readable files, one batch per change, until stopped", async () => {
  const h = new MockHost({ manifest: manifest(`read = ["**/*.rs", ".env"]`) });
  const batches: FileEvent[][] = [];
  await h.activate({
    activate(ctx: Context) {
      const w = ctx.onFiles("src/**/*.rs", (_ctx, events) => void batches.push(events));
      ctx.command("stop", () => w.stop());
    },
  });
  await h.createFile("src/lib.rs", "fn a() {}");
  await h.changeFile("src/lib.rs", "fn b() {}");
  await h.createFile("build.rs", "");
  await h.createFile("src/notes.txt", "");
  await h.deleteFile("src/lib.rs");
  const uri = h.uri("src/lib.rs");
  assert.deepEqual(batches, [[{ change: "created", uri }], [{ change: "changed", uri }], [{ change: "deleted", uri }]]);
  assert.equal(h.read("src/lib.rs"), undefined);
  await h.command("stop");
  await h.createFile("src/main.rs", "");
  assert.equal(batches.length, 3);
  h.dispose();
});

test("a watch without read is refused with the line to add", async () => {
  const h = new MockHost({ manifest: manifest(`net = ["api.example.com"]`) });
  await h.activate({
    activate(ctx: Context) {
      ctx.command("watch", () => {
        try {
          ctx.onFiles("src/*.rs", () => {});
          return "watching";
        } catch (e) {
          assert.ok(e instanceof DeniedError);
          return e.fix;
        }
      });
    },
  });
  assert.equal(await h.command("watch"), 'add to [permissions]:  read = ["src/*.rs"]');
  assert.equal(h.denied().length, 1);
  h.dispose();
});

test("a component without the watch import refuses with the fix", () => {
  // `cyx` imports `watch` only when the manifest grants `read`; without it the SDK refuses before reaching the host.
  const bindings = { log: { log() {} } } as unknown as Bindings;
  const ext = {
    activate(ctx: Context) {
      assert.throws(() => ctx.onDocuments(() => {}), (e) => e instanceof DeniedError && e.permission === 'read = ["<glob>"]');
    },
  };
  start(ext, bindings).activate({ reason: "command" });
});

test("openInEditor runs core:editor.open with the uri, or with the selection", async () => {
  const h = new MockHost({ manifest: manifest(`read = ["src/**"]\ncommands = ["core:editor.open"]`) });
  await h.activate({
    activate(ctx: Context) {
      ctx.command("open", async () => {
        await ctx.openInEditor(h.uri("src/a.ts"));
        await ctx.openInEditor(h.uri("src/a.ts"), { start: { line: 4, character: 2 }, end: { line: 4, character: 9 } });
      });
    },
  });
  await h.command("open");
  const args = h.calls().filter((c) => c.name === "commands/execute").map((c) => c.args);
  assert.deepEqual(args, [
    ["core:editor.open", JSON.stringify(h.uri("src/a.ts"))],
    ["core:editor.open", JSON.stringify({ uri: h.uri("src/a.ts"), selection: { start: { line: 4, character: 2 }, end: { line: 4, character: 9 } } })],
  ]);
  h.dispose();
});

test("net-from-setting: a host the user allowed is reached, another is theirs to allow", async () => {
  const h = new MockHost({ manifest: manifest(`net-from-setting = "server"\nsecrets = true\nwhy = "Talks to your server."`) });
  h.http("GET", "https://ghe.example.com/api/v3/user", 200, { login: "ada" });
  await h.activate({
    activate(ctx: Context) {
      ctx.command("get", async (_ctx, url: string) => {
        try {
          return await (await fetch(url)).json();
        } catch (e) {
          assert.ok(e instanceof DeniedError);
          return e.message;
        }
      });
    },
  });
  const url = "https://ghe.example.com/api/v3/user";
  assert.equal(
    await h.command("get", url),
    'denied: the manifest does not declare net-from-setting host = "ghe.example.com"\n   = fix: the user has not allowed "ghe.example.com": they add it on the extension page with Allow Host…',
  );
  h.allowHost("ghe.example.com");
  assert.deepEqual(await h.command("get", url), { login: "ada" });
  h.dispose();
});
