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

test("hints the latest version after each dependency in view", async () => {
  const host = new MockHost();
  host.http("GET", "https://crates.io/api/v1/crates/anyhow", 200, { crate: { max_version: "1.0.99" } });
  host.http("GET", "https://crates.io/api/v1/crates/ghost", 404, {});
  await host.activate(extension);
  const text = '[package]\nname = "x"\n\n[dependencies]\nanyhow = "1"\nghost = "0.1"\n';
  const doc = { uri: "file:///work/Cargo.toml", language: "toml", version: 1, text };
  const view = (from: number, to: number) => ({ start: { line: from, character: 0 }, end: { line: to, character: 0 } });
  // `name` is in [package], not a dependency; `ghost` is not on crates.io.
  assert.deepEqual(await host.inlayHints(doc, view(0, 7)), [{ position: { line: 4, character: 12 }, label: "latest 1.0.99" }]);
  // Only the lines in view are asked about, and a crate is fetched once.
  assert.deepEqual(await host.inlayHints(doc, view(5, 7)), []);
  assert.deepEqual(await host.inlayHints({ ...doc, uri: "file:///work/other.toml" }, view(0, 7)), []);
  assert.deepEqual(host.fetched().map((r) => r.url), ["https://crates.io/api/v1/crates/anyhow", "https://crates.io/api/v1/crates/ghost"]);
  host.dispose();
});
