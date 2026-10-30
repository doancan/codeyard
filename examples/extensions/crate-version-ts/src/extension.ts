// Crate Version Ts: a command that uses the global `fetch`. Requests go through Codeyard, which checks the host
// against `net` in the manifest; a refused request rejects with an error that says what to add.

import type { Context } from "@codeyard/extension";

interface CrateInfo {
  crate: { max_version: string; max_stable_version?: string | null };
}

export function activate(ctx: Context) {
  ctx.command("latest", async (ctx) => {
    const name = (await ctx.input({ prompt: "Crate name" }))?.trim();
    if (!name) return null;
    // crates.io asks every client to say who it is.
    const res = await fetch(`https://crates.io/api/v1/crates/${encodeURIComponent(name)}`, {
      headers: { "user-agent": "codeyard-crate-version-example" },
    });
    if (res.status === 404) {
      ctx.notice(`There is no crate named ${name} on crates.io.`);
      return null;
    }
    if (!res.ok) throw new Error(`crates.io answered ${res.status}`);
    const info = (await res.json()) as CrateInfo;
    const version = info.crate.max_stable_version ?? info.crate.max_version;
    ctx.notice(`${name} ${version}`);
    return version;
  });
}
