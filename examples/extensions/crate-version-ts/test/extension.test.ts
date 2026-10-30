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

test("hovering a dependency in Cargo.toml shows its latest version", async () => {
  const host = new MockHost();
  host.http("GET", "https://crates.io/api/v1/crates/serde", 200, { crate: { max_version: "1.0.228" } });
  await host.activate(extension);
  const doc = { uri: "file:///work/Cargo.toml", language: "toml", version: 1, text: '[dependencies]\nserde = "1"\n' };
  assert.deepEqual(await host.hover(doc, { line: 1, character: 2 }), { contents: "**serde**: the latest version on crates.io is 1.0.228" });
  // On the version, or in another file, there is nothing to say.
  assert.equal(await host.hover(doc, { line: 1, character: 9 }), undefined);
  assert.equal(await host.hover({ ...doc, uri: "file:///work/other.toml" }, { line: 1, character: 2 }), undefined);
  host.dispose();
});
