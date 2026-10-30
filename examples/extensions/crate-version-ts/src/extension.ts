// Crate Version Ts: a command that uses the global `fetch`, and a hover provider for Cargo.toml that uses it too.
// Requests go through Codeyard, which checks the host against `net` in the manifest; a refused request rejects with an
// error that says what to add.

import type { Context } from "@codeyard/extension";

interface CrateInfo {
  crate: { max_version: string; max_stable_version?: string | null };
}

/** The latest version of a crate, or null when crates.io does not know it. */
async function latest(name: string): Promise<string | null> {
  // crates.io asks every client to say who it is.
  const res = await fetch(`https://crates.io/api/v1/crates/${encodeURIComponent(name)}`, {
    headers: { "user-agent": "codeyard-crate-version-example" },
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`crates.io answered ${res.status}`);
  const info = (await res.json()) as CrateInfo;
  return info.crate.max_stable_version ?? info.crate.max_version;
}

/** Versions already looked up: a hover must answer quickly, so each crate is asked for once. */
const known = new Map<string, string | null>();

export function activate(ctx: Context) {
  ctx.command("latest", async (ctx) => {
    const name = (await ctx.input({ prompt: "Crate name" }))?.trim();
    if (!name) return null;
    const version = await latest(name);
    ctx.notice(version ? `${name} ${version}` : `There is no crate named ${name} on crates.io.`);
    return version;
  });
  ctx.hover(async (_ctx, document, position) => {
    if (!document.uri.endsWith("Cargo.toml")) return undefined;
    // A dependency line: `name = "1.0"` or `name = { version = "1.0" }`.
    const line = document.text.split("\n")[position.line] ?? "";
    const name = /^\s*([A-Za-z0-9_-]+)\s*=/.exec(line)?.[1];
    if (!name || position.character > line.indexOf("=")) return undefined;
    if (!known.has(name)) known.set(name, await latest(name));
    const version = known.get(name);
    return version ? { contents: `**${name}**: the latest version on crates.io is ${version}` } : undefined;
  });
}
