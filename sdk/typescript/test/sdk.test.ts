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
