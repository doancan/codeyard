// Crate Version Ts: a command that uses the global `fetch`. Requests go through Codeyard, which checks the host
// against `net` in the manifest; a refused request rejects with an error that says what to add.
//
// Inlay hints: in a `Cargo.toml`, each dependency line in view (`serde = "1.0"`) gets a hint with the latest version.
// Codeyard asks only for the lines in view, again after an edit; the versions are kept, so each crate is fetched once.

import type { Context, InlayHint } from "@codeyard/extension";

interface CrateInfo {
  crate: { max_version: string; max_stable_version?: string | null };
}

/** The latest version of each crate asked about, or null when crates.io has no such crate. */
const latest = new Map<string, Promise<string | null>>();

async function fetchLatest(name: string): Promise<string | null> {
  // crates.io asks every client to say who it is.
  const res = await fetch(`https://crates.io/api/v1/crates/${encodeURIComponent(name)}`, {
    headers: { "user-agent": "codeyard-crate-version-example" },
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`crates.io answered ${res.status}`);
  const info = (await res.json()) as CrateInfo;
  return info.crate.max_stable_version ?? info.crate.max_version;
}

function latestOf(name: string): Promise<string | null> {
  let v = latest.get(name);
  if (!v) {
    v = fetchLatest(name);
    // A failed fetch is tried again next time.
    v.catch(() => latest.delete(name));
    latest.set(name, v);
  }
  return v;
}

export function activate(ctx: Context) {
  ctx.command("latest", async (ctx) => {
    const name = (await ctx.input({ prompt: "Crate name" }))?.trim();
    if (!name) return null;
    const version = await latestOf(name);
    if (version === null) {
      ctx.notice(`There is no crate named ${name} on crates.io.`);
      return null;
    }
    ctx.notice(`${name} ${version}`);
    return version;
  });

  ctx.inlayHints(async (_ctx, document, range) => {
    if (!document.uri.endsWith("/Cargo.toml")) return [];
    const lines = document.text.split("\n");
    // Which table each line is in: only dependency tables count.
    let table = "";
    const wanted: { line: number; name: string; end: number }[] = [];
    lines.forEach((text, line) => {
      const header = /^\s*\[([^\]]+)\]/.exec(text);
      if (header) table = header[1]!.trim();
      else if (line >= range.start.line && line < range.end.line && /dependencies$/.test(table)) {
        const dep = /^\s*([A-Za-z0-9_-]+)\s*=/.exec(text);
        if (dep) wanted.push({ line, name: dep[1]!, end: text.length });
      }
    });
    const hints: InlayHint[] = [];
    for (const w of wanted) {
      const version = await latestOf(w.name).catch(() => null);
      if (version) hints.push({ position: { line: w.line, character: w.end }, label: `latest ${version}` });
    }
    return hints;
  });
}
