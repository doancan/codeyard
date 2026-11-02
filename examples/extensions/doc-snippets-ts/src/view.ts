// views.md, "Build it with the SDK".

import type { Context } from "@codeyard/extension";

declare const ctx: Context;

import { ui } from "@codeyard/extension";

ctx.view("deployments", () =>
  ui.column(
    [
      ui.heading("Deployments", 2),
      ui.button("refresh", "Refresh", { icon: "refresh-cw" }),
      ui.list("list", "Deployments", [ui.text("web: ready", { tone: "success", key: "web" }), ui.text("api: building", { tone: "warning", key: "api" })], {
        selection: "one",
      }),
    ],
    { gap: 2 },
  ),
);
