import assert from "node:assert/strict";
import { test } from "node:test";

import { MockHost } from "@codeyard/extension/testing";
import * as extension from "../src/extension.ts";

test("counts clicks", async () => {
  const host = new MockHost();
  await host.activate(extension);
  await host.viewEvent("main", { click: "add" });
  await host.viewEvent("main", { click: "add" });
  assert.equal((await host.render("main")).children[1].text, "2");
  await host.command("reset");
  assert.equal((await host.render("main")).children[1].text, "0");
  assert.deepEqual(host.invalidated(), ["main", "main", "main"]);
});
