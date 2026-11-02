import assert from "node:assert/strict";
import { test } from "node:test";

import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/extension.ts";

test("says hello", async () => {
  const host = new MockHost();             // checks calls against ./codeyard-extension.toml
  await host.activate(extension);
  host.answerInput("Ada");
  await host.command("hello");
  assert.deepEqual(host.notices(), ["Hello, Ada!"]);
});
