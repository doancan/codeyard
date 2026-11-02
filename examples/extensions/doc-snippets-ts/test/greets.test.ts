// developer-guide.md, "Test and debug".

import assert from "node:assert/strict";
import { test } from "node:test";

import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/first-command.ts";

test("greets", async () => {
  const host = new MockHost();           // from @codeyard/extension/testing
  await host.activate(extension);
  host.answerInput("Ada");
  await host.command("hello");
  assert.deepEqual(host.notices(), ["Hello, Ada!"]);
});
