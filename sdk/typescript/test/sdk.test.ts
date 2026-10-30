// The SDK against its own mock host: every kind of call, refusals with their fix, fetch with streamed bodies, timers on
// the virtual clock, and stack traces in errors.

import assert from "node:assert/strict";
import { test } from "node:test";

import { CodeyardError, DeniedError, RELOAD_STATE_KEY, type Context } from "../src/index.ts";
import { describe as describeError, stackFrames } from "../src/errors.ts";
import { start } from "../src/guest.ts";
import { MockHost } from "../src/testing/index.ts";
import { parseToml } from "../src/testing/toml.ts";
import { globMatch } from "../src/testing/permissions.ts";
import type { Bindings } from "../src/wit.ts";

const MANIFEST = `
[extension]
id          = "you.tools"   # a comment
version     = "0.1.0"
name        = "Tools"
license     = "MIT"
description = """Many
lines"""

[code]
entry = "src/extension.ts"

[permissions]
net  = ["api.example.com", "*.github.com"]
read = ["**/*.json"]
run  = ["git"]
why  = 'Reads JSON, asks GitHub.'

[settings.interval]
type        = "number"
default     = 60
description = "Seconds."

[[commands]]
id    = "a"
title = "A"
`;

const host = () => new MockHost({ manifest: MANIFEST });

test("a command asks, stores and notifies", async () => {
  const h = host();
  await h.activate({
    activate(ctx: Context) {
      ctx.command("greet", async (ctx) => {
        const name = (await ctx.input({ prompt: "Your name?" })) ?? "world";
        const count = (ctx.storage.getJson<number>("count") ?? 0) + 1;
        ctx.storage.setJson("count", count);
        ctx.notice(`Hello, ${name}!`);
        return { count, interval: ctx.setting<number>("interval") };
      });
    },
  });
  h.answerInput("Ada");
  assert.deepEqual(await h.command("greet"), { count: 1, interval: 60 });
  h.dismissInput();
  assert.deepEqual(await h.command("greet"), { count: 2, interval: 60 });
  assert.deepEqual(h.notices(), ["Hello, Ada!", "Hello, world!"]);
  assert.deepEqual(h.storage(), { count: "2" });
  await assert.rejects(h.command("missing"), /no command 'missing' is registered/);
  h.dispose();
});

test("ctx.state survives a hot reload, and only a reload", async () => {
  const h = host();
  const ext = {
    activate(ctx: Context) {
      ctx.command("tally", (ctx) => {
        const n = (ctx.state.get<number>("tally") ?? 0) + 1;
        ctx.state.set("tally", n);
        return { n, reason: ctx.activationReason };
      });
    },
  };
  await h.activate(ext);
  await h.command("tally");
  assert.deepEqual(await h.command("tally"), { n: 2, reason: "command" });
  await h.reload(ext);
  assert.deepEqual(await h.command("tally"), { n: 3, reason: "reload" });
  // The snapshot is read once.
  assert.equal(h.storage()[RELOAD_STATE_KEY], undefined);
  await h.deactivate();
  assert.equal(h.storage()[RELOAD_STATE_KEY], '{"tally":3}');
  await h.activate(ext);
  assert.deepEqual(await h.command("tally"), { n: 1, reason: "command" });
  assert.equal(h.storage()[RELOAD_STATE_KEY], undefined);
  h.dispose();
});

test("a refused fetch rejects with the manifest line to add", async () => {
  const h = host();
  await h.activate({
    activate(ctx) {
      ctx.command("go", async () => {
        try {
          await fetch("https://other.example.org/x");
          return "sent";
        } catch (e) {
          assert.ok(e instanceof DeniedError);
          return e.message;
        }
      });
    },
  });
  assert.equal(await h.command("go"), 'denied: the manifest does not declare net = ["other.example.org"]\n   = fix: add to [permissions]:  net = ["other.example.org"]');
  assert.equal(h.denied()[0]?.fix, 'add to [permissions]:  net = ["other.example.org"]');
  h.dispose();
});

test("the global fetch reaches fixtures, with headers, JSON and a streamed body", async () => {
  const h = host();
  h.http("GET", "https://api.github.com/user/repos", 200, [{ name: "codeyard" }]);
  h.http("POST", "https://api.example.com/echo", 201, "chunk", { "x-id": "7" });
  await h.activate({
    activate(ctx) {
      ctx.command("repos", async () => {
        const r = await fetch("https://api.github.com/user/repos", { headers: { accept: "application/json" }, auth: "github" } as RequestInit);
        return { ok: r.ok, type: r.headers.get("Content-Type"), repos: await r.json() };
      });
      ctx.command("echo", async () => {
        const r = await ctx.fetch("https://api.example.com/echo", { method: "post", body: JSON.stringify({ a: 1 }) });
        const chunks: string[] = [];
        for await (const c of r.chunks()) chunks.push(new TextDecoder().decode(c));
        return { status: r.status, id: r.headers.get("x-id"), chunks };
      });
    },
  });
  // `auth = ["github"]` is not declared: the signed request is refused, with its line.
  await assert.rejects(h.command("repos"), (e: unknown) => e instanceof CodeyardError && /auth = \["github"\]/.test(String(e)));
  const h2 = new MockHost({ manifest: MANIFEST.replace('run  = ["git"]', 'run  = ["git"]\nauth = ["github"]') });
  h2.http("GET", "https://api.github.com/user/repos", 200, [{ name: "codeyard" }]);
  h2.http("POST", "https://api.example.com/echo", 201, "chunk", { "x-id": "7" });
  await h2.activate({
    activate(ctx) {
      ctx.command("repos", async () => {
        const r = await fetch("https://api.github.com/user/repos", { auth: "github" } as RequestInit);
        return { ok: r.ok, type: r.headers.get("Content-Type"), repos: await r.json() };
      });
      ctx.command("echo", async () => {
        const r = await ctx.fetch("https://api.example.com/echo", { method: "post", body: JSON.stringify({ a: 1 }) });
        return { status: r.status, id: r.headers.get("x-id"), text: await r.text() };
      });
    },
  });
  assert.deepEqual(await h2.command("repos"), { ok: true, type: "application/json; charset=utf-8", repos: [{ name: "codeyard" }] });
  assert.deepEqual(await h2.command("echo"), { status: 201, id: "7", text: "chunk" });
  assert.deepEqual(
    h2.fetched().map((f) => [f.method, f.url, f.body, f.auth]),
    [
      ["GET", "https://api.github.com/user/repos", "", "github"],
      ["POST", "https://api.example.com/echo", '{"a":1}', undefined],
    ],
  );
  h2.dispose();
});

test("timers fire on the virtual clock; views draw and handle events", async () => {
  const h = host();
  let ticks = 0;
  await h.activate({
    activate(ctx) {
      ctx.every(60, async () => {
        ticks++;
      });
      ctx.view("main", async (ctx) => ({ type: "text", text: String(ctx.storage.getJson<number>("n") ?? 0) }));
      ctx.onViewEvent("main", async (ctx) => {
        ctx.storage.setJson("n", (ctx.storage.getJson<number>("n") ?? 0) + 1);
        ctx.invalidate("main");
      });
    },
  });
  await h.advance(150);
  assert.equal(ticks, 2);
  assert.deepEqual(h.timers(), [[1, 60]]);
  await h.viewEvent("main", { click: "add" });
  assert.deepEqual(await h.render("main"), { type: "text", text: "1" });
  assert.deepEqual(h.invalidated(), ["main"]);
  h.dispose();
});

test("hover and completion answer for a position", async () => {
  const h = host();
  await h.activate({
    activate(ctx) {
      ctx.hover(async (_ctx, doc, pos) => {
        const line = doc.text.split("\n")[pos.line] ?? "";
        return line.includes("TODO") ? { contents: `**TODO** on line ${pos.line + 1}` } : undefined;
      });
      ctx.completion(async (_ctx, _doc, _pos, trigger) =>
        trigger === "." ? { items: [{ label: "len", kind: "method", insertText: "len()$0", snippet: true }], incomplete: true } : [{ label: "todo" }],
      );
    },
  });
  const doc = { uri: "file:///work/a.ts", language: "typescript", version: 1, text: "let a = 1;\n// TODO\n" };
  assert.deepEqual(await h.hover(doc, { line: 1, character: 3 }), { contents: "**TODO** on line 2" });
  assert.equal(await h.hover(doc, { line: 0, character: 3 }), undefined);
  assert.deepEqual(await h.completion(doc, { line: 0, character: 0 }), {
    items: [{ label: "todo", kind: "text", snippet: false, additionalEdits: [] }],
    incomplete: false,
  });
  const dot = await h.completion(doc, { line: 0, character: 0 }, ".");
  assert.equal(dot.incomplete, true);
  assert.deepEqual(dot.items[0], { label: "len", kind: "method", insertText: "len()$0", snippet: true, additionalEdits: [] });
  h.dispose();
});

test("files, processes and progress go through the permissions", async () => {
  const h = host();
  h.file("apps/vercel.json", '{"a":1}');
  h.process("git", ["status"], { stdout: "clean\n" });
  await h.activate({
    activate(ctx) {
      ctx.command("go", async (ctx) => {
        const json = JSON.parse(await ctx.readText(h.uri("apps/vercel.json")));
        const found = await ctx.findFiles("**/*.json");
        const out = await ctx.withProgress("Checking…", () => ctx.run({ program: "git", args: ["status"] }));
        await assert.rejects(ctx.readText(h.uri(".env")), DeniedError);
        await assert.rejects(ctx.run({ program: "rm" }), /run = \["rm"\]/);
        return { json, found, code: out.code, stdout: new TextDecoder().decode(out.stdout) };
      });
    },
  });
  assert.deepEqual(await h.command("go"), { json: { a: 1 }, found: ["file:///work/apps/vercel.json"], code: 0, stdout: "clean\n" });
  assert.deepEqual(h.progress(), []);
  assert.deepEqual(
    h.denied().map((d) => d.permission),
    ['read = [".env"]', 'run = ["rm"]'],
  );
  h.dispose();
});

test("code lenses run their command with their arguments; hints cover the range asked", async () => {
  const h = host();
  const lines = (text: string, from: number, to: number) => text.split("\n").map((l, n) => [n, l] as const).slice(from, to);
  await h.activate({
    activate(ctx) {
      ctx.command("echo", (_ctx, args) => args);
      ctx.codeLenses((_ctx, doc, range) => [
        ...lines(doc.text, range.start.line, range.end.line)
          .filter(([, l]) => l.startsWith("fn "))
          .map(([n]) => ({ range: { start: { line: n, character: 0 }, end: { line: n, character: 0 } }, command: { title: "Run", command: "echo", args: { line: n } } })),
        { id: "later", range: range },
      ]);
      ctx.resolveCodeLens((_ctx, _doc, lens) => ({ ...lens, command: { title: `Resolved ${lens.id}`, command: "echo", args: lens.id } }));
      ctx.inlayHints((_ctx, doc, range) =>
        lines(doc.text, range.start.line, range.end.line).map(([n, l]) => ({ position: { line: n, character: l.length }, label: `${l.length} chars` })),
      );
    },
  });
  const doc = { uri: "file:///work/a.rs", language: "rust", version: 1, text: "fn a() {}\nlet x = 1;\nfn b() {}\n" };
  const range = (from: number, to: number) => ({ start: { line: from, character: 0 }, end: { line: to, character: 0 } });
  const lenses = await h.codeLenses(doc, range(1, 3));
  assert.equal(lenses.length, 2, "only the lines asked for");
  assert.equal(lenses[0]!.range.start.line, 2);
  assert.deepEqual(await h.clickCodeLens(lenses[0]!), { line: 2 }, "the command runs with the lens's arguments");
  await assert.rejects(h.clickCodeLens(lenses[1]!), /resolve it first/);
  const resolved = await h.resolveCodeLens(doc, lenses[1]!);
  assert.equal(resolved.command?.title, "Resolved later");
  assert.equal(await h.clickCodeLens(resolved), "later");
  const hints = await h.inlayHints(doc, range(0, 2));
  assert.deepEqual(
    hints.map((x) => x.label),
    ["9 chars", "10 chars"],
  );
  h.dispose();
});

test("a thrown error answers with its message and stack", async () => {
  const h = host();
  await h.activate({
    activate(ctx) {
      ctx.command("boom", async function boom() {
        throw new Error("it broke");
      });
    },
  });
  await assert.rejects(h.command("boom"), (e: Error) => /^it broke\n {4}at /.test(e.message) && e.message.includes("sdk.test.ts:"));
  h.dispose();
});

test("an interface the component does not import refuses the call with its fix", async () => {
  const responses: unknown[] = [];
  const bindings = {
    log: { log() {} },
    requests: { respond: (_id: bigint, r: unknown) => void responses.push(r) },
    tickets: { cancel() {} },
  } as unknown as Bindings;
  const guest = start(
    {
      activate(ctx: Context) {
        ctx.command("go", () => ctx.readText("file:///work/a.json"));
      },
    },
    bindings,
  );
  guest.activate({ reason: "command:go" });
  guest.handle(1n, { tag: "command", val: ["go", ""] });
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(responses, [{ tag: "err", val: { tag: "denied", val: { permission: 'read = ["**/a.json"]', why: "not-declared" } } }]);
});

test("stack frames from V8 and SpiderMonkey", () => {
  assert.deepEqual(stackFrames("Error: x\n    at greet (file:///a/b.js:3:9)\n    at file:///a/c.js:1:2"), ["greet (file:///a/b.js:3:9)", "<anonymous> (file:///a/c.js:1:2)"]);
  assert.deepEqual(stackFrames("greet@extension.js:3:9\n@extension.js:10:1\n"), ["greet (extension.js:3:9)", "<anonymous> (extension.js:10:1)"]);
  const e = new TypeError("bad");
  e.stack = "f@extension.js:1:2";
  assert.equal(describeError(e), "TypeError: bad\n    at f (extension.js:1:2)");
});

test("the manifest reader", () => {
  const m = parseToml(MANIFEST);
  assert.equal((m.extension as Record<string, unknown>).description, "Many\nlines");
  assert.deepEqual((m.permissions as Record<string, unknown>).net, ["api.example.com", "*.github.com"]);
  assert.equal(((m.settings as Record<string, Record<string, unknown>>).interval as Record<string, unknown>).default, 60);
  assert.equal(((m.commands as unknown[])[0] as Record<string, unknown>).id, "a");
  assert.deepEqual(parseToml('a = { b = [1, 2,], c = "\\u00e9" }\n'), { a: { b: [1, 2], c: "é" } });
  assert.ok(globMatch("**/*.json", "apps/vercel.json") && globMatch("*.json", "a/b.json") && !globMatch("src/*.ts", "src/a/b.ts"));
});
