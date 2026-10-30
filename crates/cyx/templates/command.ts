// TITLE: one command. `npm test` runs test/extension.test.ts against a mock Codeyard; `cyx dev` runs it for real.

import type { Context } from "@codeyard/extension";

export function activate(ctx: Context) {
  // `hello` is the command's id in codeyard-extension.toml.
  ctx.command("hello", async (ctx) => {
    const name = (await ctx.input({ prompt: "Your name?" }))?.trim() || "world";
    // Storage keeps its values between sessions.
    const count = (ctx.storage.getJson<number>("greetings") ?? 0) + 1;
    ctx.storage.setJson("greetings", count);
    ctx.notice(`Hello, ${name}! (${count} so far)`);
    return count;
  });
}
