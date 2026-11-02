// Short snippets of developer-guide.md and typescript.md, each in a function that gives it a `ctx`.

import { fetch, type Context } from "@codeyard/extension";

/** developer-guide.md, "Your first command": state kept across a hot reload. */
export function reloadState(ctx: Context) {
  const n = (ctx.state.get<number>("clicks") ?? 0) + 1;      // TypeScript
  ctx.state.set("clicks", n);
}

/** developer-guide.md, "Quick pick". */
export function quickPick(ctx: Context) {
  ctx.command("pick-env", async (ctx) => {
    const envs = ["production", "preview", "development"];
    const i = await ctx.quickPick(envs, { placeholder: "Environment" });   // the index, or undefined
    if (i !== undefined) ctx.storage.setJson("env", envs[i]);
  });
}

declare function restartTimer(seconds: number): void;

/** developer-guide.md, "Settings". */
export function settings(ctx: Context) {
  const seconds = ctx.setting<number>("interval") ?? 60;
  ctx.onSettingsChanged(async (ctx, keys) => {
    if (keys.includes("interval")) restartTimer(ctx.setting<number>("interval") ?? 60);
  });
  return seconds;
}

/** developer-guide.md, "Call an API with the user's GitHub login". */
export async function githubLogin() {
  const res = await fetch("https://api.github.com/user/repos", { auth: "github" });
  const repos = await res.json();
  return repos;
}

/** typescript.md, "fetch and npm packages". */
export async function signedFetch() {
  const res = await fetch("https://api.github.com/user/repos", { auth: "github" });   // auth = ["github"]
  return res;
}

/** developer-guide.md, "Diagnostics". */
export function diagnostics(ctx: Context) {
  ctx.command("find-todos", async (ctx) => {
    const doc = await ctx.activeDocument();          // a command the user started may read it without `read`
    if (!doc) return;
    const diagnostics = doc.text.split("\n").flatMap((text, line) => {
      const col = text.indexOf("TODO");
      if (col < 0) return [];
      const range = { start: { line, character: col }, end: { line, character: col + 4 } };
      return [{ range, severity: "information" as const, message: "TODO left here" }];
    });
    ctx.setDiagnostics(doc.uri, diagnostics);
  });
}

/** developer-guide.md, "Long work and processes". */
export async function longWork(ctx: Context) {
  await ctx.withProgress("Building…", async () => {
    const build = ctx.spawn({ program: "vercel", args: ["build"] });        // needs run = ["vercel"]
    for await (const chunk of build) if ("stdout" in chunk) ctx.log("info", new TextDecoder().decode(chunk.stdout));
    await build.wait();
  });
}
