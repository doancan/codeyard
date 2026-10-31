// developer-guide.md, "Your first command".

// src/extension.ts
import type { Context } from "@codeyard/extension";

export function activate(ctx: Context) {
  ctx.command("hello", async (ctx) => {
    const name = await ctx.input({ prompt: "Your name?" });
    if (name) ctx.notice(`Hello, ${name}!`);
  });
}
