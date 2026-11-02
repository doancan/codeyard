// Codeyard's rules for one call, as the app, the extension host and `cyx test` apply them (crates/ext-manifest:
// glob.rs, allow.rs, calls.rs). This is a copy for the mock host; test/rule-cases.json holds cases judged by the Rust
// rules, and test/rules.test.ts checks this copy against every one, so the two cannot drift.

import { CALL_BYTES } from "../limits.ts";

export { resultTooLarge } from "../limits.ts";

/** The limits the mock enforces, from crates/ext-manifest/src/limits.rs (checked against rule-cases.json). */
export const LIMITS = {
  CALL_BYTES,
  DATA_DIR_BYTES: 50 * 1024 * 1024,
  ITEM_LEN: 256,
  DIAGNOSTIC_FILES: 1000,
  DIAGNOSTIC_MESSAGE_LEN: 4096,
  MARKS_PER_FILE: 1000,
  STATUS_TEXT_LEN: 64,
  TIMER_MIN_SECONDS: 30,
} as const;

/** Wildcard matching in linear time: `isStar` swallows any run (empty too), `same` matches one item. */
function wildcard<P, N>(p: P[], n: N[], isStar: (x: P) => boolean, same: (a: P, b: N) => boolean): boolean {
  let pi = 0;
  let ni = 0;
  let star: [number, number] | undefined;
  while (ni < n.length) {
    if (pi < p.length && isStar(p[pi]!)) {
      star = [pi, ni];
      pi += 1;
    } else if (pi < p.length && same(p[pi]!, n[ni]!)) {
      pi += 1;
      ni += 1;
    } else if (star) {
      pi = star[0] + 1;
      ni = star[1] + 1;
      star = [star[0], star[1] + 1];
    } else {
      return false;
    }
  }
  while (pi < p.length && isStar(p[pi]!)) pi += 1;
  return pi === p.length;
}

/** One path part against one glob part: `*` and `?`, by character. */
export function nameMatches(glob: string, name: string): boolean {
  return wildcard([...glob.replace(/\/+$/, "")], [...name], (c) => c === "*", (c, x) => c === "?" || c === x);
}

/** A workspace-relative path against a glob, whole: `**` is zero or more parts; `main.rs` is the file at the top only. */
export function pathMatches(glob: string, relPath: string): boolean {
  const p = glob.replace(/^(\.\/)+/, "").replace(/^\/+|\/+$/g, "").split("/");
  const n = relPath.replace(/^\/+|\/+$/g, "").split("/");
  return wildcard(p, n, (s) => s === "**", nameMatches);
}

/** One of `globs` allows `rel`; `.env*` and `.git/` must be named by a glob part, never reached by `**`. */
export function globsAllow(globs: string[], rel: string): boolean {
  const namesHidden = (s: string) => s.split("/").some((part) => part.startsWith(".env") || part === ".git");
  const hidden = namesHidden(rel);
  return globs.some((g) => pathMatches(g, rel) && (!hidden || namesHidden(g)));
}

function percentDecode(s: string): string | undefined {
  const bytes: number[] = [];
  const raw = new TextEncoder().encode(s);
  for (let i = 0; i < raw.length; i++) {
    if (raw[i] === 0x25) {
      const hex = s.slice(i + 1, i + 3);
      if (!/^[0-9a-fA-F]{2}$/.test(hex)) return undefined;
      bytes.push(parseInt(hex, 16));
      i += 2;
    } else {
      bytes.push(raw[i]!);
    }
  }
  let out: string;
  try {
    out = new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(bytes));
  } catch {
    return undefined;
  }
  // A decoded NUL or control character in a path is never legitimate.
  return /\p{Cc}/u.test(out) ? undefined : out;
}

/** A `file://` URI or a path → an absolute path with `/` separators and no `.`/`..` parts, or undefined. */
export function normalize(uriOrPath: string): string | undefined {
  const raw = uriOrPath.startsWith("file://") ? uriOrPath.slice("file://".length) : uriOrPath;
  const decoded = percentDecode(raw);
  if (decoded === undefined) return undefined;
  let s = decoded.replace(/\\/g, "/");
  if (/^\/[A-Za-z]:/.test(s)) s = s.slice(1);
  let prefix: string;
  let rest: string;
  if (s.startsWith("/")) {
    prefix = "/";
    rest = s.slice(1);
  } else if (/^[A-Za-z]:$/.test(s)) {
    prefix = `${s}/`;
    rest = "";
  } else if (/^[A-Za-z]:\//.test(s)) {
    prefix = `${s.slice(0, 2)}/`;
    rest = s.slice(3);
  } else {
    return undefined;
  }
  const parts: string[] = [];
  for (const p of rest.split("/")) {
    if (p === "" || p === ".") continue;
    if (p === "..") {
      if (parts.length === 0) return undefined;
      parts.pop();
    } else {
      parts.push(p);
    }
  }
  return prefix + parts.join("/");
}

/** The path of `uri` relative to the folder that holds it (`""` for the folder), or undefined when it is in none. */
export function relative(folders: string[], uri: string): string | undefined {
  const path = normalize(uri);
  if (path === undefined) return undefined;
  for (const f of folders) {
    const root = normalize(f)?.replace(/\/+$/, "");
    if (root === undefined || !path.startsWith(root)) continue;
    const rest = path.slice(root.length);
    if (rest === "") return "";
    if (rest.startsWith("/")) return rest.slice(1);
  }
  return undefined;
}

/** A refusal no manifest entry can lift: `read file:///etc/passwd (cannot be granted: <reason>)`. */
export function ungrantableLine(what: string, reason: string): string {
  return `${what} (cannot be granted: ${reason})`;
}

/** The `permission` of the denial for a `kind` (`read`, `write`) of a path outside every workspace folder. */
export function outsideLine(kind: string, path: string): string {
  return ungrantableLine(`${kind} ${path}`, "the file is outside the workspace, and extensions reach only files inside its folders");
}

/** The `permission` of the denial for a request to an IP address, which `net` cannot list. */
export function ipAddressLine(host: string): string {
  return ungrantableLine(`net ${host}`, "it is an IP address, and net lists host names only; connect to the server by its host name");
}

export type UrlProblem = "not-https" | "ip-literal" | "invalid";

function shorten(url: string): string {
  return url.length <= 200 ? url : `${url.slice(0, 200)}…`;
}

function isNumericHost(host: string): boolean {
  const num = (l: string) => l !== "" && (/^[0-9]+$/.test(l) || /^0x[0-9a-fA-F]*$/.test(l));
  const labels = host.split(".");
  return /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.test(host) || labels.every(num) || /^[0-9]*$/.test(labels[labels.length - 1]!);
}

/**
 * A URL Codeyard fetches: its host and port, or why not, in Codeyard's words. `host` of a refusal is the IP address as
 * written for `ip-literal` (for `ipAddressLine`), else empty.
 */
export function checkUrl(input: string): { host: string; port: number } | { problem: UrlProblem; message: string; host: string } {
  let url = input.trim();
  const hash = url.indexOf("#");
  if (hash >= 0) url = url.slice(0, hash);
  const shown = shorten(url);
  const bad = (problem: UrlProblem, why: string, host = "") => ({ problem, message: `${shown}: ${why}`, host });
  const schemeEnd = url.indexOf("://");
  if (schemeEnd < 0) return bad("invalid", "is not a URL; write it as https://host/path");
  if (url.slice(0, schemeEnd).toLowerCase() !== "https") return bad("not-https", "only https:// URLs are allowed; use https://");
  const authority = url.slice(schemeEnd + 3).split(/[/?]/)[0] ?? "";
  if (authority.includes("@")) return bad("invalid", "must not contain a user name or password; send credentials in a header");
  const ipLiteral = (host: string) =>
    bad("ip-literal", "names an IP address; extensions connect only to host names listed in `net`, so use the host name", host.toLowerCase());
  if (authority.startsWith("[")) return ipLiteral(authority.slice(0, authority.indexOf("]") + 1) || authority);
  const colon = authority.lastIndexOf(":");
  const raw = colon >= 0 ? authority.slice(0, colon) : authority;
  const port = colon >= 0 ? authority.slice(colon + 1) : undefined;
  let host = raw.replace(/[A-Z]/g, (c) => c.toLowerCase());
  if (host.endsWith(".")) host = host.slice(0, -1);
  if (host === "") return bad("invalid", "has no host");
  if (!/^[a-z0-9.-]+$/.test(host) || host.includes("..") || host.endsWith(".")) {
    return bad("invalid", "has an invalid host name; use letters, digits, '-' and dots (xn-- for other scripts)");
  }
  if (isNumericHost(host)) return ipLiteral(host);
  if (port === undefined) return { host, port: 443 };
  if (port === "") return bad("invalid", "has an empty port");
  if (!/^[0-9]+$/.test(port)) return bad("invalid", "has an invalid port");
  const n = Number(port);
  if (n === 0) return bad("invalid", "has port 0");
  if (n > 65535) return bad("invalid", "has an invalid port");
  return { host, port: n };
}

/** Controls, bidirectional overrides and zero-width characters (ext_manifest::is_deceptive). */
function deceptive(s: string): boolean {
  return /[\p{Cc}​-‏‪-‮⁠-⁤⁦-⁩؜﻿­]/u.test(s);
}

/** `window.set-status-item` as Codeyard checks it: the text as shown, or the error. `declared` undefined skips the id check. */
export function checkStatusItem(
  declared: string[] | undefined,
  id: string,
  text: string,
  tooltip: string | undefined,
): { shown: string } | { error: string } {
  if (id === "") return { error: "a status item needs an id: the local id of a [[status-bar]] entry" };
  if (declared && !declared.includes(id)) return { error: `status item "${id}" is not declared; add [[status-bar]] with id = "${id}" to the manifest` };
  const shown = text.trim();
  if (shown === "" || [...shown].length > LIMITS.STATUS_TEXT_LEN || deceptive(shown)) {
    return {
      error: `status item "${id}": its text must be 1 to ${LIMITS.STATUS_TEXT_LEN} visible characters, without control, bidirectional or zero-width characters`,
    };
  }
  if (tooltip !== undefined && ([...tooltip].length > LIMITS.ITEM_LEN || deceptive(tooltip))) {
    return { error: `status item "${id}": its tooltip must be at most ${LIMITS.ITEM_LEN} characters, without control, bidirectional or zero-width characters` };
  }
  return { shown };
}

/**
 * A diagnostic's message as Codeyard shows it: trimmed, undefined when empty or with a character that could disguise
 * what it says (line breaks and tabs are fine), cut to DIAGNOSTIC_MESSAGE_LEN characters with a `…`.
 */
export function shownDiagnosticMessage(message: string): { shown: string; cut: boolean } | undefined {
  const m = message.trim();
  if (m === "" || deceptive(m.replace(/[\n\t]/g, ""))) return undefined;
  const chars = [...m];
  return chars.length > LIMITS.DIAGNOSTIC_MESSAGE_LEN
    ? { shown: `${chars.slice(0, LIMITS.DIAGNOSTIC_MESSAGE_LEN).join("")}…`, cut: true }
    : { shown: m, cut: false };
}

/**
 * Why an extension may not set diagnostics on a file, or undefined when it may. `declared` is the `languages` of each
 * `[[diagnostics]]` entry (empty: every language; undefined skips the check, as without a manifest).
 */
export function checkDiagnosticsTarget(declared: string[][] | undefined, language: string, reachable: boolean, otherFiles: number): string | undefined {
  if (declared) {
    if (declared.length === 0) return "the manifest declares no [[diagnostics]]";
    if (!declared.some((l) => l.length === 0 || l.includes(language))) return `[[diagnostics]] does not list its language, ${language}`;
  }
  if (!reachable) return "it is neither in the workspace nor open in an editor";
  if (otherFiles >= LIMITS.DIAGNOSTIC_FILES) return `it has diagnostics on ${otherFiles} files already, the most one extension may; clear some first`;
  return undefined;
}

const BY_EXTENSION: Record<string, string> = {
  rs: "rust", c: "c", h: "c", cc: "cpp", cpp: "cpp", cxx: "cpp", hpp: "cpp", hh: "cpp", hxx: "cpp", go: "go", py: "python", pyi: "python",
  ts: "typescript", mts: "typescript", cts: "typescript", tsx: "typescriptreact", js: "javascript", mjs: "javascript", cjs: "javascript",
  jsx: "javascriptreact", java: "java", cs: "csharp", php: "php", phtml: "php", rb: "ruby", rake: "ruby", gemspec: "ruby", ru: "ruby",
  kt: "kotlin", kts: "kotlin", swift: "swift", lua: "lua", sh: "shellscript", bash: "shellscript", html: "html", htm: "html", css: "css",
  scss: "scss", less: "less", json: "json", jsonc: "jsonc", yaml: "yaml", yml: "yaml", toml: "toml",
};

/** The built-in language id of a file name (`a.rs` is `rust`), undefined for one no built-in language claims. */
export function builtinLanguage(path: string): string | undefined {
  const name = path.slice(path.lastIndexOf("/") + 1);
  if (["Gemfile", "Rakefile", "Podfile", "Vagrantfile", "Guardfile"].includes(name)) return "ruby";
  const dot = name.lastIndexOf(".");
  return dot < 0 ? undefined : BY_EXTENSION[name.slice(dot + 1).toLowerCase()];
}

/** The language id Codeyard gives a file: a built-in one, else one of the extension's own `[[languages]]`, else `plaintext`. */
export function languageId(own: { id: string; extensions: string[]; filenames: string[] }[], path: string): string {
  const builtin = builtinLanguage(path);
  if (builtin) return builtin;
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  const ext = dot < 0 ? undefined : name.slice(dot + 1).toLowerCase();
  const hit = own.find((l) => l.filenames.includes(name)) ?? (ext === undefined ? undefined : own.find((l) => l.extensions.some((e) => e.toLowerCase() === ext)));
  return hit?.id ?? "plaintext";
}

/** What `core:editor.open` says about an argument it cannot take (`CoreCommand::bad_argument`). */
export const EDITOR_OPEN_ARGUMENT = "core:editor.open takes a `file://` URI, or `{\"uri\": …, \"selection\": range}` (required)";

/** The file `core:editor.open`'s argument names: a URI, or `{uri, selection?}` whose selection has two positions of non-negative whole numbers. */
export function editorOpenTarget(arg: unknown): string | undefined {
  if (typeof arg === "string") return arg;
  if (typeof arg !== "object" || arg === null || Array.isArray(arg)) return undefined;
  const { uri, selection } = arg as { uri?: unknown; selection?: unknown };
  if (typeof uri !== "string") return undefined;
  if (selection === undefined || selection === null) return uri;
  const position = (p: unknown) => {
    const { line, character } = (p ?? {}) as { line?: unknown; character?: unknown };
    return [line, character].every((n) => Number.isInteger(n) && (n as number) >= 0 && (n as number) <= 0xffffffff);
  };
  const { start, end } = selection as { start?: unknown; end?: unknown };
  return position(start) && position(end) ? uri : undefined;
}
