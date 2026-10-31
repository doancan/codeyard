// Hello View Ts: a view with a button, and a command that resets it. `npm test` runs test/extension.test.ts against a mock
// Codeyard; `cyx dev` shows the view in the sidebar.

import { ui, type Context } from "@codeyard/extension";

const clicks = (ctx: Context) => ctx.storage.getJson<number>("clicks") ?? 0;

export function activate(ctx: Context) {
  // The view's tree (docs/extensions/views.md). Codeyard draws it with the user's theme.
  ctx.view("main", (ctx) => ui.column([ui.heading("Clicks", 2), ui.text(String(clicks(ctx))), ui.button("add", "Add one")], { gap: 2, pad: 2 }));
  // A click on the button arrives as { click: { path: ["add"] } }: count it and draw the view again.
  ctx.onViewEvent("main", async (ctx, event) => {
    if (event.click?.path?.[0] === "add") {
      ctx.storage.setJson("clicks", clicks(ctx) + 1);
      ctx.invalidate("main");
    }
  });
  ctx.command("reset", async (ctx) => {
    ctx.storage.delete("clicks");
    ctx.invalidate("main");
  });
}
