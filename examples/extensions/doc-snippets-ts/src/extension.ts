// typescript.md, "Your first command".

import type { Context } from "@codeyard/extension";

export function activate(ctx: Context) {
  ctx.command("hello", async (ctx) => {
    const name = (await ctx.input({ prompt: "Your name?" })) || "world";
    ctx.notice(`Hello, ${name}!`);
  });
}
