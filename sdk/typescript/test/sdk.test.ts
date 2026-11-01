// The SDK against its own mock host: every kind of call, refusals with their fix, fetch with streamed bodies, timers on
// the virtual clock, and stack traces in errors.

import assert from "node:assert/strict";
import { test } from "node:test";

import { CodeyardError, DeniedError, RELOAD_STATE_KEY, ui, type Context, type ViewNode } from "../src/index.ts";
import { describe as describeError, stackFrames } from "../src/errors.ts";
import { start } from "../src/guest.ts";
import { MockHost } from "../src/testing/index.ts";
import { parseToml } from "../src/testing/toml.ts";
import { pathMatches } from "../src/testing/rules.ts";
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
      ctx.view("main", async (ctx) => ui.text(String(ctx.storage.getJson<number>("n") ?? 0)));
      // The `{ type: … }` shorthand the templates once wrote (#182), which Codeyard refuses.
      ctx.view("shorthand", () => ({ type: "column", children: [{ type: "text", text: "2" }] }) as unknown as ViewNode);
      ctx.onViewEvent("main", async (ctx) => {
        ctx.storage.setJson("n", (ctx.storage.getJson<number>("n") ?? 0) + 1);
        ctx.invalidate("main");
      });
    },
  });
  await h.advance(150);
  assert.equal(ticks, 2);
  assert.deepEqual(h.timers(), [[1, 60]]);
  await h.viewEvent("main", { click: { path: ["add"] } });
  assert.deepEqual(await h.render("main"), { element: { text: { text: "1" } } });
  assert.deepEqual(h.invalidated(), ["main"]);
  // A tree or an event Codeyard would refuse fails the test, with Codeyard's reason.
  await assert.rejects(h.render("shorthand"), /Codeyard would not draw view shorthand: the view is malformed: unknown field `type`/);
  await assert.rejects(h.viewEvent("main", { click: "add" }), /Codeyard never sends \{"click":"add"\}/);
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
  // As in Codeyard, a glob is matched against the whole path: `*.json` is a file at the top of the folder.
  assert.ok(pathMatches("**/*.json", "apps/vercel.json") && !pathMatches("*.json", "a/b.json") && !pathMatches("src/*.ts", "src/a/b.ts"));
});

test("a pull request provider answers every query through the signed fetch", async () => {
  const h = new MockHost({
    manifest: `
[extension]
id = "you.gitea"
version = "0.1.0"
name = "Gitea"
license = "MIT"

[code]
entry = "src/extension.ts"

[permissions]
auth = ["gitea"]

[[pull-requests]]
id    = "gitea"
label = "Gitea"
hosts = ["codeberg.org"]
auth  = "gitea"
`,
  });
  const api = "https://codeberg.org/api/v1/repos/ada/app";
  h.http("GET", `${api}/pulls?state=open`, 200, [
    { number: 7, title: "Fix it", user: { login: "ada" }, draft: true, head: { ref: "fix" }, base: { ref: "main" }, updated_at: "2026-01-02T00:00:00Z", html_url: "https://codeberg.org/ada/app/pulls/7" },
  ]);
  h.http("POST", `${api}/issues/7/comments`, 201, {});
  const comments: string[] = [];
  await h.activate({
    activate(ctx: Context) {
      ctx.pullRequests("gitea", {
        async list(ctx, remote) {
          const r = await ctx.fetchAs(remote.account!, `https://${remote.host}/api/v1/repos/${remote.path}/pulls?state=open`);
          const pulls = (await r.json()) as any[];
          return pulls.map((p) => ({
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
        details: (_ctx, _remote, id) => ({
          summary: { id, reference: `#${id}`, title: "Fix it", author: "ada", state: "open", sourceBranch: "fix", targetBranch: "main" },
          description: "It **was** broken.",
          checks: [{ name: "ci", state: "success" }],
        }),
        files: () => [
          { path: "src/a.ts", status: "modified" },
          { path: "src/b.ts", oldPath: "src/old.ts", status: "renamed" },
        ],
        contents: (_ctx, _remote, _id, file) => (file.status === "added" ? { head: "new\n" } : { base: "old\n", head: "new\n" }),
        comments: () => [{ author: "bob", body: "Looks good", created: 1000, path: "src/a.ts", line: 3 }],
        async addComment(ctx, remote, id, body) {
          await ctx.fetchAs(remote.account!, `https://${remote.host}/api/v1/repos/${remote.path}/issues/${id}/comments`, { method: "POST", body: JSON.stringify({ body }) });
          comments.push(body);
        },
        checkout: (_ctx, _remote, id) => ({ ref: `refs/pull/${id}/head`, branch: `pr-${id}` }),
      });
    },
  });
  const remote = { provider: "gitea", name: "origin", url: "git@codeberg.org:ada/app.git", host: "codeberg.org", path: "ada/app", account: "gitea" };

  assert.deepEqual(await h.listPullRequests(remote), [
    { id: "7", reference: "#7", title: "Fix it", author: "ada", state: "draft", sourceBranch: "fix", targetBranch: "main", updated: Date.parse("2026-01-02T00:00:00Z"), url: "https://codeberg.org/ada/app/pulls/7" },
  ]);
  // The host signs the request; the SDK adds no authorization header of its own.
  const [listed] = h.fetched();
  assert.equal(listed?.auth, "gitea");
  assert.ok(!listed?.headers.some(([k]) => k.toLowerCase() === "authorization"));

  // The raw answer is WIT: u64 as BigInt.
  const raw = await h.pullRequests({ tag: "list", remote });
  assert.equal(raw.tag === "list" && raw.val[0]?.updated, BigInt(Date.parse("2026-01-02T00:00:00Z")));

  assert.deepEqual(await h.pullRequestDetails(remote, "7"), {
    summary: { id: "7", reference: "#7", title: "Fix it", author: "ada", state: "open", sourceBranch: "fix", targetBranch: "main", updated: 0 },
    description: "It **was** broken.",
    checks: [{ name: "ci", state: "success" }],
    labels: [],
  });
  assert.deepEqual(await h.pullRequestFiles(remote, "7"), [
    { path: "src/a.ts", status: "modified" },
    { path: "src/b.ts", oldPath: "src/old.ts", status: "renamed" },
  ]);
  assert.deepEqual(await h.pullRequestContents(remote, "7", { path: "src/c.ts", status: "added" }), { head: "new\n" });
  assert.deepEqual(await h.pullRequestContents(remote, "7", { path: "src/a.ts", status: "modified" }), { base: "old\n", head: "new\n" });
  assert.deepEqual(await h.pullRequestComments(remote, "7"), [{ author: "bob", body: "Looks good", created: 1000, path: "src/a.ts", line: 3 }]);
  await h.addPullRequestComment(remote, "7", "Thanks!");
  assert.deepEqual(comments, ["Thanks!"]);
  assert.deepEqual(
    h.fetched().map((f) => [f.method, f.url, f.body, f.auth]),
    [
      ["GET", `${api}/pulls?state=open`, "", "gitea"],
      ["GET", `${api}/pulls?state=open`, "", "gitea"],
      ["POST", `${api}/issues/7/comments`, '{"body":"Thanks!"}', "gitea"],
    ],
  );
  assert.deepEqual(await h.checkoutPullRequest(remote, "7"), { ref: "refs/pull/7/head", branch: "pr-7" });

  // A provider id nothing registered.
  await assert.rejects(
    h.listPullRequests({ ...remote, provider: "gitlab" }),
    (e: unknown) =>
      e instanceof CodeyardError && e.kind === "not-found" && e.message.includes(`no pull request provider 'gitlab' is registered; call ctx.pullRequests("gitlab", …) in activate`),
  );
  assert.deepEqual(h.denied(), []);
  h.dispose();
});

test("a pull request provider without its optional handlers", async () => {
  const h = MockHost.allowAll();
  await h.activate({
    activate(ctx: Context) {
      ctx.pullRequests("bare", {
        list: () => [],
        details: () => ({ summary: { id: "1", reference: "!1", title: "T", author: "a", state: "merged", sourceBranch: "x", targetBranch: "main" }, description: "" }),
        files: () => [],
        contents: () => ({}),
      });
    },
  });
  const remote = { provider: "bare", name: "origin", url: "https://git.example.com/a/b.git", host: "git.example.com", path: "a/b" };
  assert.deepEqual(await h.listPullRequests(remote), []);
  assert.deepEqual(await h.pullRequestContents(remote, "1", { path: "a", status: "deleted" }), {});
  assert.deepEqual(await h.pullRequestComments(remote, "1"), []);
  await assert.rejects(h.addPullRequestComment(remote, "1", "hi"), (e: unknown) => e instanceof CodeyardError && e.kind === "invalid" && /'bare' does not take comments/.test(e.message));
  await assert.rejects(h.checkoutPullRequest(remote, "1"), (e: unknown) => e instanceof CodeyardError && e.kind === "invalid" && /'bare' has no checkout handler/.test(e.message));
  h.dispose();

  // The required handlers are checked when the provider is registered.
  const h2 = MockHost.allowAll();
  await assert.rejects(
    h2.activate({
      activate(ctx: Context) {
        ctx.pullRequests("half", { list: () => [] } as never);
      },
    }),
    /the pull request provider 'half' needs a details\(ctx, …\) handler/,
  );
  h2.dispose();
});

test("auth.fetch signs only for the provider's hosts, and never with an authorization of the extension's own", async () => {
  const h = new MockHost({
    manifest: `
[permissions]
auth = ["acme", "github"]

[[oauth]]
id            = "acme"
name          = "Acme"
authorize-url = "https://acme.example.com/oauth/authorize"
token-url     = "https://acme.example.com/oauth/token"
client-id     = "x"
hosts         = ["*.acme.example.com"]
`,
  });
  for (const url of ["https://api.acme.example.com/me", "https://evil.example.org/collect", "https://api.github.com/user", "https://example.com/x"]) h.http("GET", url, 200, {});
  await h.activate({
    activate(ctx: Context) {
      ctx.command("get", async (ctx, [provider, url, headers]) => (await ctx.fetchAs(provider, url, { headers })).status);
    },
  });
  assert.equal(await h.command("get", ["acme", "https://api.acme.example.com/me"]), 200);
  assert.equal(await h.command("get", ["github", "https://api.github.com/user"]), 200);
  await assert.rejects(h.command("get", ["acme", "https://evil.example.org/collect"]), (e: unknown) => e instanceof DeniedError);
  await assert.rejects(h.command("get", ["github", "https://example.com/x"]), (e: unknown) => e instanceof DeniedError);
  assert.deepEqual(
    h.denied().map((d) => d.permission),
    ['[[oauth]] acme hosts = ["evil.example.org"]', '[[oauth]] github hosts = ["example.com"]'],
  );
  // #183: the fix names the `[[oauth]]` table, not the `auth` line the manifest already has.
  assert.equal(h.denied()[0]?.fix, 'add "evil.example.org" to hosts in the [[oauth]] table with id = "acme"');
  assert.match(h.denied()[1]?.fix ?? "", /with fetch \(net\) instead/);
  await assert.rejects(
    h.command("get", ["acme", "https://api.acme.example.com/me", { authorization: "Bearer mine" }]),
    (e: unknown) => e instanceof CodeyardError && e.kind === "invalid" && /added by Codeyard/.test(e.message),
  );
  assert.equal(h.fetched().length, 2, "only the two allowed requests were sent");
  h.dispose();
});

test("sign-in outcomes: declined and signed out, as Codeyard reports them", async () => {
  const h = new MockHost({
    manifest: `
[permissions]
auth = ["acme"]

[[oauth]]
id            = "acme"
name          = "Acme"
authorize-url = "https://acme.example.com/oauth/authorize"
token-url     = "https://acme.example.com/oauth/token"
client-id     = "x"
hosts         = ["api.acme.example.com"]
`,
  });
  h.http("GET", "https://api.acme.example.com/me", 200, {});
  await h.activate({
    activate(ctx: Context) {
      ctx.command("me", async (ctx) => {
        const who = await ctx.session("acme");
        return { label: who.label, status: (await ctx.fetchAs("acme", "https://api.acme.example.com/me")).status };
      });
      ctx.command("fetch-only", async (ctx) => (await ctx.fetchAs("acme", "https://api.acme.example.com/me")).status);
    },
  });
  assert.deepEqual(await h.command("me"), { label: "test-user", status: 200 });

  h.declineSignIn("acme");
  await assert.rejects(h.command("me"), (e: unknown) => e instanceof CodeyardError && e.kind === "cancelled");
  await assert.rejects(h.command("fetch-only"), (e: unknown) => e instanceof CodeyardError && e.kind === "invalid" && /not signed in to Acme; call auth.session with “acme” first/.test(e.message));

  h.signedOut("acme");
  await assert.rejects(h.command("fetch-only"), /not signed in to Acme/);
  // The next session asks again, and this time the user agrees.
  assert.deepEqual(await h.command("me"), { label: "test-user", status: 200 });

  h.signIn("acme", "ada");
  assert.deepEqual(await h.command("me"), { label: "ada", status: 200 });
  assert.deepEqual(h.denied(), []);
  h.dispose();
});

test("an instance account is granted by its provider's entry once added, and signs its own host only", async () => {
  const h = new MockHost({
    manifest: `
[permissions]
auth = ["forge"]

[[oauth]]
id            = "forge"
name          = "Forge"
authorize-url = "https://forge.example.com/oauth/authorize"
token-url     = "https://forge.example.com/oauth/token"
client-id     = "x"
hosts         = ["forge.example.com"]
instances     = true
`,
  });
  h.http("GET", "https://git.corp.example/api/me", 200, {});
  h.http("GET", "https://forge.example.com/api/me", 200, {});
  await h.activate({
    activate(ctx: Context) {
      ctx.command("get", async (ctx, [account, url]) => {
        await ctx.session(account);
        return (await ctx.fetchAs(account, url)).status;
      });
    },
  });
  const account = "forge@git.corp.example";
  await assert.rejects(h.command("get", [account, "https://git.corp.example/api/me"]), (e: unknown) => e instanceof DeniedError && e.permission === 'auth = ["forge"]');
  h.instance("forge", "git.corp.example");
  assert.equal(await h.command("get", [account, "https://git.corp.example/api/me"]), 200);
  await assert.rejects(
    h.command("get", [account, "https://forge.example.com/api/me"]),
    (e: unknown) => e instanceof DeniedError && e.permission === '[[oauth]] forge@git.corp.example hosts = ["forge.example.com"]',
  );
  assert.equal(h.fetched().length, 1);
  h.dispose();
});
