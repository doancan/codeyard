// developer-guide.md, "Status bar item".

import type { Context } from "@codeyard/extension";

export function activate(ctx: Context) {
  ctx.setStatusItem({ id: "deploy", text: "Deploy: ready", tone: "success" });
  ctx.every(60, async (ctx) => {                                    // minimum 30 s, pauses when unfocused
    ctx.setStatusItem({ id: "deploy", text: "Deploy: checking…" });
  });
}
