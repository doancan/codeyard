// developer-guide.md, "Tree view": a module whose `ctx` and data come from elsewhere.

import type { Context } from "@codeyard/extension";

declare const ctx: Context;
declare const deployments: { id: string; name: string; state: string }[];

import { ui } from "@codeyard/extension";

ctx.view("deployments", async (ctx) =>
  ui.list(
    "deployments",
    "Deployments",
    deployments.map((d) =>
      ui.row([ui.text(d.name), ui.badge(d.state, { tone: d.state === "ready" ? "success" : "warning" })], { key: d.id }),
    ),
  ),
);

// When data changes:
ctx.invalidate("deployments");
