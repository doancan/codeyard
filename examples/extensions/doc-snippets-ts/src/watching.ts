// developer-guide.md, "Watch documents and files", and commands.md, "Run core commands".

import type { Context } from "@codeyard/extension";

/** TODO lines per file, as the watches below keep them. */
const todos = new Map<string, number>();

export function activate(ctx: Context) {
  // Open documents: those already open first, then each edit (once typing pauses) and save. Needs `read`.
  ctx.onDocuments((ctx, e) => {
    todos.set(e.uri, (e.text ?? "").split("\n").filter((l) => l.includes("TODO")).length);
  }, { text: true });

  // Files on disk, changed by Codeyard or anything else: one batch at most every second.
  ctx.onFiles("src/**/*.ts", async (ctx, events) => {
    for (const e of events) {
      if (e.change === "deleted") todos.delete(e.uri);
      else todos.set(e.uri, (await ctx.readText(e.uri)).split("\n").filter((l) => l.includes("TODO")).length);
    }
  });

  ctx.command("open-first-todo", async (ctx) => {
    const [uri] = [...todos.keys()];
    // Jump to line 3, columns 0 to 4; leave the range out to open the file at the top.
    if (uri) await ctx.openInEditor(uri, { start: { line: 2, character: 0 }, end: { line: 2, character: 4 } });
  });
}

export function counts(): Record<string, number> {
  return Object.fromEntries(todos);
}
