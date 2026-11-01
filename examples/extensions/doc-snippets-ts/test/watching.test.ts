import assert from "node:assert/strict";
import { test } from "node:test";

import { MockHost } from "@codeyard/extension/testing";
import * as watching from "../src/watching.ts";

test("counts TODOs as documents change and files appear", async () => {
  const host = new MockHost();
  await host.openDocument("src/a.ts", "typescript", "// TODO one");
  await host.activate(watching);
  await host.editDocument("src/a.ts", "// TODO one\n// TODO two");
  await host.createFile("src/b.ts", "// TODO three");
  assert.deepEqual(watching.counts(), { [host.uri("src/a.ts")]: 2, [host.uri("src/b.ts")]: 1 });
  await host.deleteFile("src/b.ts");
  await host.command("open-first-todo");
  assert.deepEqual(host.calls().at(-1)?.args[0], "core:editor.open");
  host.dispose();
});
