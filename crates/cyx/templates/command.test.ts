import assert from "node:assert/strict";
import { test } from "node:test";

import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/extension.ts";

test("says hello", async () => {
  const host = new MockHost();
  await host.activate(extension);
  host.answerInput("Ada");
  assert.equal(await host.command("hello"), 1);
  host.dismissInput();
  assert.equal(await host.command("hello"), 2);
  assert.deepEqual(host.notices(), ["Hello, Ada! (1 so far)", "Hello, world! (2 so far)"]);
});
