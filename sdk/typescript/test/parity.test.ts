// The mock refuses what Codeyard refuses: each case was found by an extension whose tests passed against the mock and
// that then failed in Codeyard.

import assert from "node:assert/strict";
import { test } from "node:test";

import { CodeyardError, type Context } from "../src/index.ts";
import { MockHost } from "../src/testing/index.ts";

const MANIFEST = `
[extension]
id = "you.parity"
version = "0.1.0"
name = "Parity"
license = "MIT"

[code]
entry = "src/extension.ts"

[permissions]
read = ["main.rs", "docs/**"]
net  = ["api.example.com"]

[[commands]]
id = "read"
title = "Read"

[[status-bar]]
id = "deploys"

[[hover]]
languages = ["rust"]

[[diagnostics]]
id = "lint"
languages = ["rust", "toml"]
`;

const extension = {
  activate(ctx: Context) {
    ctx.command("read", (ctx, uri: string) => ctx.readText(uri));
    ctx.command("find", (ctx, glob: string) => ctx.findFiles(glob));
    ctx.command("fetch", async (_ctx, url: string) => (await fetch(url)).status);
    ctx.command("status", (ctx, item: { id: string; text: string }) => ctx.setStatusItem({ ...item, tone: "neutral" }));
    ctx.command("diagnose", (ctx, a: { uri: string; messages: string[] }) =>
      ctx.setDiagnostics(a.uri, a.messages.map((message) => ({ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } }, severity: "warning", message }))),
    );
    ctx.command("clear", (ctx, uri: string) => ctx.clearDiagnostics(uri));
    ctx.command("huge", () => "x".repeat(20 << 20));
    ctx.command("open", (ctx) => ctx.openExternal("https://example.com/"));
    ctx.hover(async (ctx) => {
      await ctx.openExternal("https://example.com/");
      return undefined;
    });
  },
};

async function host(t: { after(f: () => void): void }): Promise<MockHost> {
  const h = new MockHost({ manifest: MANIFEST });
  t.after(() => h.dispose());
  await h.activate(extension);
  return h;
}

test("a file name glob is the file at the top of the folder only", async (t) => {
  const h = await host(t);
  h.file("main.rs", "fn main() {}").file("src/main.rs", "fn main() {}").file("docs/a/b.md", "# b");
  assert.equal(await h.command("read", "file:///work/main.rs"), "fn main() {}");
  await assert.rejects(h.command("read", "file:///work/src/main.rs"), /read = \["src\/main.rs"\]/);
  assert.equal(await h.command("read", "file:///work/docs/a/b.md"), "# b");
  assert.deepEqual(await h.command("find", "**/*.rs"), ["file:///work/main.rs"]);
});

test("a path out of the folder is refused however it is spelled", async (t) => {
  const h = await host(t);
  for (const uri of ["file:///work/docs/../../etc/passwd", "file:///work/docs/%2e%2e/%2e%2e/etc/passwd", "file:///etc/passwd"]) {
    await assert.rejects(h.command("read", uri), /none: this cannot be granted, the file is outside the workspace/, uri);
  }
});

test("URLs Codeyard never fetches are invalid, even with a fixture", async (t) => {
  const h = await host(t);
  h.http("GET", "http://api.example.com/zen", 200, "plain");
  h.http("GET", "https://me:pw@api.example.com/zen", 200, "creds");
  await assert.rejects(h.command("fetch", "http://api.example.com/zen"), /only https:\/\//);
  await assert.rejects(h.command("fetch", "https://me:pw@api.example.com/zen"), /user name or password/);
  await assert.rejects(h.command("fetch", "https://127.0.0.1/"), /none: this cannot be granted, it is an IP address/);
  assert.deepEqual(h.fetched(), []);
});

test("status items follow Codeyard's rule", async (t) => {
  const h = await host(t);
  await h.command("status", { id: "deploys", text: " Ready " });
  assert.equal(h.statusItems().deploys?.text, "Ready");
  await assert.rejects(h.command("status", { id: "other", text: "Ready" }), /status item "other" is not declared/);
  await assert.rejects(h.command("status", { id: "deploys", text: "y".repeat(65) }), /1 to 64 visible characters/);
});

test("an answer over the call limit fails its request", async (t) => {
  const h = await host(t);
  await assert.rejects(h.command("huge"), (e: CodeyardError) => e.kind === "invalid" && e.message.includes("the result is 20 MiB; at most 8 MiB"));
});

test("links open only from a user action", async (t) => {
  const h = await host(t);
  await h.command("open");
  const doc = { uri: "file:///work/main.rs", language: "rust", version: 1, text: "" };
  await assert.rejects(h.hover(doc, { line: 0, character: 0 }), /a user action/);
});

test("fetch is the mock's from the start, and works from the test itself", async (t) => {
  const h = new MockHost({ manifest: MANIFEST });
  t.after(() => h.dispose());
  h.http("GET", "https://api.example.com/zen", 200, "calm");
  // Before activate, straight from the test: the fixture answers, nothing reaches the network, nothing hangs.
  const r = await fetch("https://api.example.com/zen");
  assert.equal(await r.text(), "calm");
  await assert.rejects(fetch("https://api.github.com/zen"), /net = \["api.github.com"\]/);
});

test("an async activate that rejects fails the activation, and requests wait for it", async (t) => {
  const failing = new MockHost({ manifest: MANIFEST });
  t.after(() => failing.dispose());
  failing.http("GET", "https://api.example.com/config", 500, "");
  const broken = {
    async activate(ctx: Context) {
      const r = await fetch("https://api.example.com/config");
      if (!r.ok) throw new Error(`no config (${r.status})`);
      ctx.command("read", () => "never");
    },
  };
  await assert.rejects(failing.activate(broken), /activate failed: no config \(500\)/);
  assert.ok(failing.logs().some(([level, m]) => level === "error" && m.startsWith("activate failed: no config (500)")));

  const working = new MockHost({ manifest: MANIFEST });
  t.after(() => working.dispose());
  working.http("GET", "https://api.example.com/config", 200, "{}");
  await working.activate({
    async activate(ctx: Context) {
      await fetch("https://api.example.com/config");
      // Registered after an await: a request that came in meanwhile still finds it.
      ctx.command("read", () => "late");
    },
  });
  assert.equal(await working.command("read"), "late");
});

const warned = (h: MockHost, what: string) => h.logs().some(([level, m]) => level === "warn" && m.includes(what));

test("diagnostics follow Codeyard's rules", async (t) => {
  const h = await host(t);
  const messages = () => h.diagnostics("src/lib.rs").map((d) => d.message);
  await h.command("diagnose", { uri: "file:///work/src/lib.rs", messages: ["  unused import ", "", "   "] });
  assert.deepEqual(messages(), ["unused import"]);
  assert.ok(warned(h, "2 with an empty message or hidden characters dropped"), JSON.stringify(h.logs()));
  await h.command("diagnose", { uri: "file:///work/src/lib.rs", messages: ["a\u202eb", "c\u200bd", "line one\n\tline two"] });
  assert.deepEqual(messages(), ["line one\n\tline two"]);
  await h.command("diagnose", { uri: "file:///work/src/lib.rs", messages: ["ğ".repeat(5000)] });
  assert.equal([...messages()[0]!].length, 4097);
  assert.ok(messages()[0]!.endsWith("…"));
  await h.command("diagnose", { uri: "file:///work/main.go", messages: ["no"] });
  assert.deepEqual(h.diagnostics("main.go"), []);
  assert.ok(warned(h, "does not list its language, go"), JSON.stringify(h.logs()));
  await h.command("diagnose", { uri: "file:///work/notes.md", messages: ["no"] });
  assert.ok(warned(h, "does not list its language, plaintext"), "Codeyard has no markdown language id");
  await h.command("diagnose", { uri: "file:///etc/hosts.rs", messages: ["no"] });
  assert.deepEqual(h.diagnostics("file:///etc/hosts.rs"), []);
  assert.ok(warned(h, "neither in the workspace nor open in an editor"));
  h.activeDocument("file:///elsewhere/lib.rs", "rust", "fn main() {}");
  await h.command("diagnose", { uri: "file:///elsewhere/lib.rs", messages: ["fine"] });
  assert.equal(h.diagnostics("file:///elsewhere/lib.rs").length, 1);
});

test("diagnostics need a declaration and stop at a thousand files", async (t) => {
  const bare = new MockHost({ manifest: MANIFEST.replace('[[diagnostics]]\nid = "lint"\nlanguages = ["rust", "toml"]\n', "") });
  t.after(() => bare.dispose());
  await bare.activate(extension);
  await bare.command("diagnose", { uri: "file:///work/a.rs", messages: ["x"] });
  assert.deepEqual(bare.diagnostics("a.rs"), []);
  assert.ok(warned(bare, "declares no [[diagnostics]]"), JSON.stringify(bare.logs()));

  const h = await host(t);
  for (let i = 0; i < 1000; i++) await h.command("diagnose", { uri: `file:///work/f${i}.rs`, messages: ["x"] });
  await h.command("diagnose", { uri: "file:///work/one-too-many.rs", messages: ["x"] });
  assert.deepEqual(h.diagnostics("one-too-many.rs"), []);
  assert.ok(warned(h, "1000 files already"));
  await h.command("diagnose", { uri: "file:///work/f0.rs", messages: ["y"] });
  assert.equal(h.diagnostics("f0.rs")[0]?.message, "y");
  await h.command("clear", "file:///work/f1.rs");
  await h.command("diagnose", { uri: "file:///work/one-too-many.rs", messages: ["x"] });
  assert.equal(h.diagnostics("one-too-many.rs").length, 1);
});

test("an extension's own language counts for diagnostics", async (t) => {
  const h = new MockHost({
    manifest: MANIFEST.replace('languages = ["rust", "toml"]', 'languages = ["acme"]').replace("[[hover]]", '[[languages]]\nid = "acme"\nextensions = ["acme"]\n\n[[hover]]'),
  });
  t.after(() => h.dispose());
  await h.activate(extension);
  await h.command("diagnose", { uri: "file:///work/a.acme", messages: ["fine"] });
  assert.equal(h.diagnostics("a.acme").length, 1, JSON.stringify(h.logs()));
  await h.command("diagnose", { uri: "file:///work/a.rs", messages: ["no"] });
  assert.deepEqual(h.diagnostics("a.rs"), []);
  assert.ok(warned(h, "does not list its language, rust"));
});
