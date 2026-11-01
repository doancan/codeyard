// The manifest's `[permissions]` and the checks the app's broker makes, as in `ext-manifest` (allow.rs): a call the
// mock refuses is one Codeyard would refuse, with the same manifest line. Paths are matched by ./rules.ts.

import { globsAllow } from "./rules.ts";
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
  };
}

export function allowsNet(p: Permissions, host: string): boolean {
  const h = host.toLowerCase();
  return p.net.some((n) => (n.startsWith("*.") ? h.endsWith(`.${n.slice(2).toLowerCase()}`) : n.toLowerCase() === h));
}

export function allowsRead(p: Permissions, path: string): boolean {
  return globsAllow(p.read, path);
}

export function allowsWrite(p: Permissions, path: string): boolean {
  return globsAllow(p.write, path);
}
