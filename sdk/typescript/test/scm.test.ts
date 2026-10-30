// A source control provider on the mock host: its state, a file's original text, the input box check, a button.

import assert from "node:assert/strict";
import { test } from "node:test";

import { scmArgs, type Context } from "../src/index.ts";
import { MockHost } from "../src/testing/index.ts";

const MANIFEST = `
[extension]
id      = "you.toy"
version = "0.1.0"
name    = "Toy"
license = "MIT"

[code]
entry = "src/extension.ts"

[permissions]
read = [".toy/**"]
run  = ["toy"]
why  = "Tests."

[[commands]]
id    = "commit"
title = "Toy: Commit"

[[scm]]
id      = "toy"
label   = "Toy"
markers = [".toy"]
`;

const toy = {
  activate(ctx: Context) {
    ctx.scmStatus("toy", async (ctx, root) => {
      const text = await ctx.readText(`${root}/.toy/changes`);
      const resources = text
        .split("\n")
        .filter((l) => l)
        .map((path) => ({ uri: `${root}/${path}`, state: "modified" as const }));
      return {
        head: "main",
        input: { placeholder: "Message", accept: { command: "commit", title: "Commit", icon: "check" } },
        groups: [{ id: "changes", label: "Changes", resources }],
      };
    });
    ctx.scmOriginal("toy", async (ctx, uri) => {
      try {
        return await ctx.readText(uri.replace("/work/", "/work/.toy/base/"));
      } catch {
        return undefined;
      }
    });
    ctx.scmValidate("toy", (_ctx, _root, text) => (text.trim() ? undefined : "Write a message"));
    ctx.command("commit", async (ctx, args) => {
      const a = scmArgs(args);
      const out = await ctx.run({ program: "toy", args: ["commit", "-m", a.message] });
      ctx.scmRefresh(a.provider);
      return { code: out.code, files: a.uris.length };
    });
  },
};

test("a provider answers its state, original text and checks, and its button runs its command", async () => {
  const h = new MockHost({ manifest: MANIFEST });
  h.file(".toy/changes", "a.txt\nb.txt\n").file(".toy/base/a.txt", "old\n");
  h.process("toy", ["commit", "-m", "Fix"], { stdout: "" });
  await h.activate(toy);

  const state = await h.scmStatus("toy");
  assert.equal(state.head, "main");
  assert.deepEqual(
    state.groups[0]!.resources.map((r) => [r.uri, r.state]),
    [
      ["file:///work/a.txt", "modified"],
      ["file:///work/b.txt", "modified"],
    ],
  );
  assert.equal(state.groups[0]!.hideWhenEmpty, false);
  assert.equal(await h.scmOriginal("toy", "a.txt"), "old\n");
  assert.equal(await h.scmOriginal("toy", "b.txt"), undefined);
  assert.equal(await h.scmValidate("toy", " "), "Write a message");
  assert.equal(await h.scmValidate("toy", "Fix"), undefined);

  assert.deepEqual(await h.scmPress("toy", state.input.accept!, ["a.txt"], "Fix"), { code: 0, files: 1 });
  assert.deepEqual(h.ran()[0]!.args, ["commit", "-m", "Fix"]);
  assert.deepEqual(h.scmRefreshed(), ["toy"]);
  await assert.rejects(h.scmStatus("other"), /no source control state for 'other'/);
});
