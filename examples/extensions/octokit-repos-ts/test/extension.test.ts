import assert from "node:assert/strict";
import { test } from "node:test";

import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/extension.ts";

const REPOS = "https://api.github.com/user/repos?per_page=100&sort=updated";

test("lists the repositories and opens the one picked", async () => {
  const host = new MockHost();
  host.http("GET", REPOS, 200, [
    { full_name: "ada/engine", html_url: "https://github.com/ada/engine", description: "Analytical" },
    { full_name: "ada/notes", html_url: "https://github.com/ada/notes", description: null },
  ]);
  await host.activate(extension);
  host.answerPick([1]);
  assert.deepEqual(await host.command("list"), ["ada/engine", "ada/notes"]);
  // Signed for the GitHub account, never with a token of the extension's own.
  const [request] = host.fetched();
  assert.equal(request?.auth, "github");
  assert.ok(!request?.headers.some(([name]) => name === "authorization"));
  assert.deepEqual(
    host.calls().map((c) => c.name),
    ["auth/session", "auth/fetch", "window/quick-pick", "window/open-external"],
  );
  assert.deepEqual(host.calls()[3]?.args, ["https://github.com/ada/notes"]);
  host.dispose();
});

test("a GitHub error is the command's error", async () => {
  const host = new MockHost();
  host.http("GET", REPOS, 401, { message: "Bad credentials" });
  await host.activate(extension);
  await assert.rejects(host.command("list"), /Bad credentials/);
  host.dispose();
});
