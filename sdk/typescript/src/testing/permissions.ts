// The manifest's `[permissions]` and the checks the app's broker makes, as in `ext-manifest` (allow.rs): a call the
// mock refuses is one Codeyard would refuse, with the same manifest line.

import type { Table } from "./toml.ts";

export interface Permissions {
  read: string[];
  write: string[];
  net: string[];
  auth: string[];
  secrets: boolean;
  run: string[];
  clipboard: "none" | "read" | "write" | "read-write";
  terminal: boolean;
  commands: string[];
  /** `net-from-setting`: the setting whose host the user may allow on the extension page. */
  netFromSetting?: string;
}

export function permissionsOf(manifest: Table): Permissions {
  const p = (manifest.permissions ?? {}) as Table;
  const list = (k: string) => (Array.isArray(p[k]) ? (p[k] as unknown[]).map(String) : []);
  const clipboard = p.clipboard;
  return {
    read: list("read"),
    write: list("write"),
    net: list("net"),
    auth: list("auth"),
    secrets: p.secrets === true,
    run: list("run"),
    clipboard: clipboard === "read" || clipboard === "write" || clipboard === "read-write" ? clipboard : "none",
    terminal: p.terminal === true,
    commands: list("commands"),
    ...(typeof p["net-from-setting"] === "string" ? { netFromSetting: p["net-from-setting"] } : {}),
  };
}

export function allowsNet(p: Permissions, host: string): boolean {
  const h = host.toLowerCase();
  return p.net.some((n) => (n.startsWith("*.") ? h.endsWith(`.${n.slice(2).toLowerCase()}`) : n.toLowerCase() === h));
}

export function allowsRead(p: Permissions, path: string): boolean {
  return allowed(p.read, path);
}

export function allowsWrite(p: Permissions, path: string): boolean {
  return allowed(p.write, path);
}

function allowed(globs: string[], path: string): boolean {
  const rel = path.replace(/^(\.\/)+/, "").replace(/^\/+/, "");
  const guarded = rel.split("/").some((part) => part.startsWith(".env") || part === ".git");
  return globs.some((g) => globMatch(g, rel) && (!guarded || g.includes(".env") || g.includes(".git")));
}

/** `*` and `?` within one path part, `**` across parts. A glob without `/` matches the file name in any folder. */
export function globMatch(glob: string, path: string): boolean {
  const g = glob.replace(/^(\.\/)+/, "").split("/");
  const p = path.split("/");
  if (g.length === 1 && g[0] !== "**") return partMatch(g[0] as string, p[p.length - 1] ?? "");
  return partsMatch(g, p);
}

function partsMatch(g: string[], p: string[]): boolean {
  if (g.length === 0) return p.length === 0;
  const [first, ...rest] = g as [string, ...string[]];
  if (first === "**") {
    for (let i = 0; i <= p.length; i++) if (partsMatch(rest, p.slice(i))) return true;
    return false;
  }
  return p.length > 0 && partMatch(first, p[0] as string) && partsMatch(rest, p.slice(1));
}

function partMatch(g: string, s: string): boolean {
  if (g.length === 0) return s.length === 0;
  const c = g[0];
  if (c === "*") {
    for (let i = 0; i <= s.length; i++) if (partMatch(g.slice(1), s.slice(i))) return true;
    return false;
  }
  if (c === "?") return s.length > 0 && partMatch(g.slice(1), s.slice(1));
  return s[0] === c && partMatch(g.slice(1), s.slice(1));
}
