import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/extension.ts";

const LIST = "https://api.vercel.com/v6/deployments?projectId=prj_web&limit=20&teamId=team_acme";

const deployments = {
  deployments: [
    {
      uid: "dpl_2",
      name: "web",
      url: "web-2.vercel.app",
      created: Date.UTC(2026, 9, 31, 14, 5),
      state: "BUILDING",
      target: "production",
      meta: { githubCommitRef: "main", githubCommitMessage: "Ship the new header\n\nLonger text" },
    },
    { uid: "dpl_1", name: "web", url: "web-1.vercel.app", created: Date.UTC(2026, 9, 30, 9, 0), readyState: "READY", target: null },
  ],
};

/** A linked folder (`vercel link`) and a token in the keychain. */
function linked(): MockHost {
  return new MockHost().file(".vercel/project.json", JSON.stringify({ projectId: "prj_web", orgId: "team_acme" })).secret("token", "vt_secret");
}

let host: MockHost | undefined;
afterEach(async () => {
  await host?.deactivate();
  host?.dispose();
  host = undefined;
});

test("without a token it asks for one and sends nothing", async () => {
  host = new MockHost();
  await host.activate(extension);
  assert.equal(host.statusItems().deployment?.command, "set-token");
  const tree = await host.render("deployments");
  assert.deepEqual(Object.keys(tree.element), ["empty-state"]);
  assert.equal(tree.children[0].key, "set-token");
  assert.deepEqual(host.fetched(), []);
  assert.deepEqual(host.denied(), []);
});

test("shows the latest deployment in the status bar and the recent ones in the view", async () => {
  host = linked().http("GET", LIST, 200, deployments);
  await host.activate(extension);
  const item = host.statusItems().deployment!;
  assert.equal(item.text, "Vercel: Building");
  assert.equal(item.tone, "warning");
  assert.match(item.tooltip!, /web · main · 2026-10-31 14:05 UTC · https:\/\/web-2\.vercel\.app/);

  // The token goes in the header, to api.vercel.com only.
  const [sent] = host.fetched();
  assert.equal(sent!.url, LIST);
  assert.deepEqual(sent!.headers, [["authorization", "Bearer vt_secret"]]);

  const tree = await host.render("deployments");
  const table = tree.children.find((c: any) => c.key === "deployments");
  assert.deepEqual(table.element.table.columns, ["State", "Target", "Branch", "Commit", "Created", "Link"]);
  assert.deepEqual(
    table.children.map((r: any) => r.key),
    ["dpl_2", "dpl_1"],
  );
  const [first] = table.children;
  assert.deepEqual(first.children[0].element, { badge: { text: "Building", tone: "warning" } });
  assert.equal(first.children[3].element.text.text, "Ship the new header");
  assert.deepEqual(first.children[5], { key: "open", element: { link: { label: "Open", url: "https://web-2.vercel.app" } } });
  assert.deepEqual(table.children[1].children[0].element, { badge: { text: "Ready", tone: "success" } });
});

test("refreshes on the timer, from the view and with the command", async () => {
  host = linked().http("GET", LIST, 200, deployments);
  await host.activate(extension);
  await host.advance(60);
  await host.viewEvent("deployments", { click: { path: ["refresh"] } });
  await host.command("refresh");
  assert.equal(host.fetched().length, 4);
  assert.ok(host.invalidated().length >= 4);
});

test("a refused token shows in red with what to do", async () => {
  host = linked().http("GET", LIST, 401, { error: { code: "forbidden" } });
  await host.activate(extension);
  assert.equal(host.statusItems().deployment?.tone, "danger");
  await host.command("refresh");
  assert.deepEqual(host.notices(), ["Vercel refused the access token (401). Run Vercel: Set Access Token."]);
  const tree = await host.render("deployments");
  assert.deepEqual(tree.children[0].element, { banner: { text: "Vercel refused the access token (401). Run Vercel: Set Access Token.", tone: "danger" } });
});

test("set-token checks the token with Vercel, keeps it and refreshes", async () => {
  host = new MockHost()
    .file(".vercel/project.json", JSON.stringify({ projectId: "prj_web", orgId: "team_acme" }))
    .http("GET", "https://api.vercel.com/v2/user", 200, { user: { username: "ada" } })
    .http("GET", LIST, 200, deployments);
  await host.activate(extension);
  host.answerInput("  vt_new  ");
  await host.command("set-token");
  assert.deepEqual(host.notices(), ["Vercel Status is signed in as ada."]);
  assert.deepEqual(host.secrets(), { token: "vt_new" });
  assert.equal(host.statusItems().deployment?.text, "Vercel: Building");
  assert.deepEqual(
    host.fetched().map((f) => f.headers),
    [[["authorization", "Bearer vt_new"]], [["authorization", "Bearer vt_new"]]],
  );
  // The input box hid what was typed.
  const input = host.calls().find((c) => c.name === "window/input-box");
  assert.equal((input?.args[0] as { password: boolean }).password, true);
});

test("a token Vercel refuses is not kept", async () => {
  host = new MockHost().http("GET", "https://api.vercel.com/v2/user", 403, {});
  await host.activate(extension);
  host.answerInput("bad");
  await host.command("set-token");
  assert.deepEqual(host.notices(), ["Vercel did not accept that token (403); it was not saved."]);
  assert.deepEqual(host.secrets(), {});
});

test("the project setting wins over the linked folder, and a personal project has no team", async () => {
  host = new MockHost()
    .secret("token", "vt_secret")
    .file(".vercel/project.json", JSON.stringify({ projectId: "prj_web", orgId: "user_123" }))
    .http("GET", "https://api.vercel.com/v6/deployments?projectId=prj_web&limit=20", 200, { deployments: [] });
  await host.activate(extension);
  assert.equal(host.statusItems().deployment?.text, "Vercel: no deployments");

  host.http("GET", "https://api.vercel.com/v6/deployments?projectId=docs&limit=20", 200, deployments);
  await host.changeSetting("project", "docs");
  assert.equal(host.fetched().at(-1)?.url, "https://api.vercel.com/v6/deployments?projectId=docs&limit=20");
  assert.equal(host.statusItems().deployment?.text, "Vercel: Building");
});

test("forget-token deletes the token", async () => {
  host = linked().http("GET", LIST, 200, deployments);
  await host.activate(extension);
  await host.command("forget-token");
  assert.deepEqual(host.secrets(), {});
  assert.equal(host.statusItems().deployment?.command, "set-token");
  assert.deepEqual(Object.keys((await host.render("deployments")).element), ["empty-state"]);
});
