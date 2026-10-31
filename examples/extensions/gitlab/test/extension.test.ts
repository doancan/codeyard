import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { afterEach, test } from "node:test";

import type { Remote } from "@codeyard/extension";
import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/extension.ts";

const remote: Remote = {
  provider: "gitlab",
  name: "origin",
  url: "git@gitlab.com:acme/web/app.git",
  host: "gitlab.com",
  path: "acme/web/app",
  account: "gitlab",
};
const PROJECT = "https://gitlab.com/api/v4/projects/acme%2Fweb%2Fapp";

const mr = {
  iid: 7,
  title: "Fix the parser",
  description: "Fixes **the parser**.",
  state: "opened",
  draft: false,
  author: { username: "ada" },
  source_branch: "fix-parser",
  target_branch: "main",
  created_at: "2026-10-30T09:00:00Z",
  updated_at: "2026-10-31T14:05:00.000Z",
  web_url: "https://gitlab.com/acme/web/app/-/merge_requests/7",
  labels: ["bug", "parser"],
  head_pipeline: { id: 99, status: "running", web_url: "https://gitlab.com/acme/web/app/-/pipelines/99" },
  diff_refs: { base_sha: "b45e000000", head_sha: "4ead000000", start_sha: "57a2000000" },
};

let host: MockHost | undefined;
afterEach(() => {
  host?.dispose();
  host = undefined;
});

async function started(): Promise<MockHost> {
  host = new MockHost();
  await host.activate(extension, "pull-requests:gitlab");
  return host;
}

test("lists the open merge requests through the GitLab account", async () => {
  const host = await started();
  host.http("GET", `${PROJECT}/merge_requests?state=opened&order_by=updated_at&sort=desc&per_page=100`, 200, [
    mr,
    { ...mr, iid: 8, title: "Draft: new lexer", draft: true, author: { username: "grace" }, head_pipeline: null },
  ]);
  const list = await host.listPullRequests(remote);
  assert.deepEqual(list, [
    {
      id: "7",
      reference: "!7",
      title: "Fix the parser",
      author: "ada",
      state: "open",
      sourceBranch: "fix-parser",
      targetBranch: "main",
      updated: Date.UTC(2026, 9, 31, 14, 5),
      url: "https://gitlab.com/acme/web/app/-/merge_requests/7",
    },
    {
      id: "8",
      reference: "!8",
      title: "Draft: new lexer",
      author: "grace",
      state: "draft",
      sourceBranch: "fix-parser",
      targetBranch: "main",
      updated: Date.UTC(2026, 9, 31, 14, 5),
      url: "https://gitlab.com/acme/web/app/-/merge_requests/7",
    },
  ]);
  // Signed by Codeyard for the account: the extension set no authorization header of its own.
  const [sent] = host.fetched();
  assert.equal(sent!.auth, "gitlab");
  assert.ok(!sent!.headers.some(([name]) => name.toLowerCase() === "authorization"));
  assert.deepEqual(
    host.calls().filter((c) => c.name === "auth/session").map((c) => c.args[0]),
    ["gitlab"],
  );
  assert.deepEqual(host.denied(), []);
});

test("details carry the description, the head pipeline and the labels", async () => {
  const host = await started();
  host.http("GET", `${PROJECT}/merge_requests/7`, 200, mr);
  const d = await host.pullRequestDetails(remote, "7");
  assert.equal(d.summary.reference, "!7");
  assert.equal(d.description, "Fixes **the parser**.");
  assert.deepEqual(d.checks, [{ name: "Pipeline #99", state: "pending", url: "https://gitlab.com/acme/web/app/-/pipelines/99" }]);
  assert.deepEqual(d.labels, ["bug", "parser"]);
});

test("changed files and both sides of each", async () => {
  const host = await started();
  host.http("GET", `${PROJECT}/merge_requests/7/diffs?per_page=100`, 200, [
    { old_path: "src/parse.rs", new_path: "src/parse.rs", new_file: false, renamed_file: false, deleted_file: false },
    { old_path: "src/lex.rs", new_path: "src/lexer.rs", new_file: false, renamed_file: true, deleted_file: false },
    { old_path: "NEW.md", new_path: "NEW.md", new_file: true, renamed_file: false, deleted_file: false },
    { old_path: "old.txt", new_path: "old.txt", new_file: false, renamed_file: false, deleted_file: true },
  ]);
  const files = await host.pullRequestFiles(remote, "7");
  assert.deepEqual(files, [
    { path: "src/parse.rs", status: "modified" },
    { path: "src/lexer.rs", oldPath: "src/lex.rs", status: "renamed" },
    { path: "NEW.md", status: "added" },
    { path: "old.txt", status: "deleted" },
  ]);

  host.http("GET", `${PROJECT}/merge_requests/7`, 200, mr);
  host.http("GET", `${PROJECT}/repository/files/src%2Flex.rs/raw?ref=b45e000000`, 200, "fn lex() {}\n");
  host.http("GET", `${PROJECT}/repository/files/src%2Flexer.rs/raw?ref=4ead000000`, 200, "fn lexer() {}\n");
  assert.deepEqual(await host.pullRequestContents(remote, "7", files[1]!), { base: "fn lex() {}\n", head: "fn lexer() {}\n" });

  // An added file has no base; a binary one shows as no text.
  host.http("GET", `${PROJECT}/repository/files/NEW.md/raw?ref=4ead000000`, 200, new Uint8Array([0x89, 0x50, 0x00, 0x47]));
  assert.deepEqual(await host.pullRequestContents(remote, "7", files[2]!), {});
});

test("comments, oldest first and without system notes; adding one posts a note", async () => {
  const host = await started();
  host.http("GET", `${PROJECT}/merge_requests/7/notes?sort=asc&order_by=created_at&per_page=100`, 200, [
    { body: "Looks good", system: false, created_at: "2026-10-31T10:00:00Z", author: { username: "grace" } },
    { body: "added 1 commit", system: true, created_at: "2026-10-31T10:01:00Z", author: { username: "ada" } },
    {
      body: "Off by one?",
      system: false,
      created_at: "2026-10-31T10:02:00Z",
      author: { username: "linus" },
      position: { new_path: "src/parse.rs", new_line: 12 },
    },
  ]);
  assert.deepEqual(await host.pullRequestComments(remote, "7"), [
    { author: "grace", body: "Looks good", created: Date.UTC(2026, 9, 31, 10, 0) },
    { author: "linus", body: "Off by one?", created: Date.UTC(2026, 9, 31, 10, 2), path: "src/parse.rs", line: 12 },
  ]);

  host.http("POST", `${PROJECT}/merge_requests/7/notes`, 201, { id: 1 });
  await host.addPullRequestComment(remote, "7", "Thanks!");
  const post = host.fetched().at(-1)!;
  assert.equal(post.method, "POST");
  assert.equal(post.auth, "gitlab");
  assert.deepEqual(JSON.parse(post.body), { body: "Thanks!" });
});

test("checks out the merge request's head ref, forks included", async () => {
  const host = await started();
  assert.deepEqual(await host.checkoutPullRequest(remote, "7"), { ref: "refs/merge-requests/7/head", branch: "mr-7" });
  await assert.rejects(host.checkoutPullRequest(remote, "7; rm -rf"), /not a merge request number/);
});

test("a self-managed instance uses its host and its own account", async () => {
  // The manifest's commented-out block, uncommented with a real host.
  const manifest = readFileSync("codeyard-extension.toml", "utf8")
    .replace('auth = ["gitlab"]', 'auth = ["gitlab", "gitlab-self-managed"]')
    .replace(/^# (?=\[\[|[a-z-]+ += )/gm, "")
    .replaceAll("gitlab.example.com", "git.acme.example.com");
  host = new MockHost({ manifest });
  await host.activate(extension, "pull-requests:gitlab-self-managed");
  const own: Remote = { ...remote, provider: "gitlab-self-managed", host: "git.acme.example.com", account: "gitlab-self-managed" };
  host.http("GET", "https://git.acme.example.com/api/v4/projects/acme%2Fweb%2Fapp/merge_requests?state=opened&order_by=updated_at&sort=desc&per_page=100", 200, [mr]);
  assert.equal((await host.listPullRequests(own))[0]?.reference, "!7");
  assert.equal(host.fetched()[0]?.auth, "gitlab-self-managed");
  assert.deepEqual(host.denied(), []);
});

test("an error from GitLab says what it answered", async () => {
  const host = await started();
  host.http("GET", `${PROJECT}/merge_requests/9`, 404, { message: "404 Not found" });
  await assert.rejects(host.pullRequestDetails(remote, "9"), /gitlab\.com answered 404 for .*merge_requests\/9: 404 Not found/);
});

test("sign-in names the account", async () => {
  const host = await started();
  await host.command("sign-in");
  assert.deepEqual(host.notices(), ["GitLab: signed in as test-user."]);
});
