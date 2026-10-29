// TITLE: a view with a button, and a command that resets it. `npm test` runs test/extension.test.ts against a mock
// Codeyard; `cyx dev` shows the view in the sidebar.

import type { Context } from "@codeyard/extension";

const clicks = (ctx: Context) => ctx.storage.getJson<number>("clicks") ?? 0;

export function activate(ctx: Context) {
  // The view's element tree (docs/extensions/commands.md#views). Codeyard draws it with the user's theme.
  ctx.view("main", async (ctx) => ({
    type: "column",
    children: [
      { type: "heading", text: "Clicks" },
      { type: "text", text: String(clicks(ctx)) },
      { type: "button", key: "add", label: "Add one" },
    ],
  }));
  // A click on the button: count it and draw the view again.
  ctx.onViewEvent("main", async (ctx) => {
    ctx.storage.setJson("clicks", clicks(ctx) + 1);
    ctx.invalidate("main");
  });
  ctx.command("reset", async (ctx) => {
    ctx.storage.delete("clicks");
    ctx.invalidate("main");
  });
}
