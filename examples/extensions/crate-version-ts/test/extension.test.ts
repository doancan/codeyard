import assert from "node:assert/strict";
import { test } from "node:test";

import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/extension.ts";

test("shows the latest version", async () => {
  const host = new MockHost();
  host.http("GET", "https://crates.io/api/v1/crates/serde", 200, { crate: { max_version: "2.0.0-rc.1", max_stable_version: "1.0.228" } });
  host.http("GET", "https://crates.io/api/v1/crates/nope", 404, {});
  await host.activate(extension);
  host.answerInput("serde");
  assert.equal(await host.command("latest"), "1.0.228");
  assert.deepEqual(host.fetched()[0]?.headers, [["user-agent", "codeyard-crate-version-example"]]);
  host.answerInput("nope");
  assert.equal(await host.command("latest"), null);
  assert.deepEqual(host.notices(), ["serde 1.0.228", "There is no crate named nope on crates.io."]);
  host.dispose();
});
