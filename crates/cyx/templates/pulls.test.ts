import assert from "node:assert/strict";
import { test } from "node:test";

import type { Remote } from "@codeyard/extension";
import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/extension.ts";

const API = "https://forge.example.com/api/v1/repos/ada/app";
const pull = {
  number: 3,
  title: "Fix the parser",
  state: "open",
  user: { login: "ada" },
  head: { ref: "fix" },
  base: { ref: "main" },
  body: "Fixes it.",
  labels: [{ name: "bug" }],
};
const remote: Remote = { provider: "pulls", name: "origin", url: "git@forge.example.com:ada/app.git", host: "forge.example.com", path: "ada/app", account: "forge" };

test("lists and shows pull requests through the account", async () => {
  const host = new MockHost();
  host.http("GET", `${API}/pulls?state=open&sort=recentupdate&limit=50`, 200, [pull]);
  host.http("GET", `${API}/pulls/3`, 200, pull);
  await host.activate(extension);
  const [first] = await host.listPullRequests(remote);
  assert.deepEqual([first?.reference, first?.title, first?.author], ["#3", "Fix the parser", "ada"]);
  const details = await host.pullRequestDetails(remote, "3");
  assert.deepEqual([details.description, details.labels], ["Fixes it.", ["bug"]]);
  assert.deepEqual(host.denied(), [], "every call is allowed by the manifest");
  host.dispose();
});

test("a declined sign-in is an error, not a list", async () => {
  const host = new MockHost();
  await host.activate(extension);
  host.declineSignIn("forge");
  await assert.rejects(host.listPullRequests(remote), /cancelled/);
  host.dispose();
});
