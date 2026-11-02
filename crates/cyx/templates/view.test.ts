import assert from "node:assert/strict";
import { test } from "node:test";

import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/extension.ts";

test("counts clicks", async () => {
  const host = new MockHost();
  await host.activate(extension);
  const click = { click: { path: ["add"] } };
  await host.viewEvent("main", click);
  await host.viewEvent("main", click);
  // `render` fails if Codeyard would refuse the tree.
  const count = async () => (await host.render("main")).children[1].element.text.text;
  assert.equal(await count(), "2");
  await host.command("reset");
  assert.equal(await count(), "0");
  assert.deepEqual(host.invalidated(), ["main", "main", "main"]);
});
