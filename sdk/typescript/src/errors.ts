// Errors. A refused call rejects with a `DeniedError`, and its message says what to add to the manifest.

import type { WitError, Why } from "./wit.ts";

export type { Why };

export type ErrorKind = "denied" | "not-found" | "cancelled" | "timeout" | "invalid" | "other";

/** An error from Codeyard (`types.error`), or one the SDK raises itself (`other`). */
export class CodeyardError extends Error {
  readonly kind: ErrorKind;

  constructor(kind: ErrorKind, message: string) {
    super(message);
    this.name = "CodeyardError";
    this.kind = kind;
  }

  static notFound(what: string): CodeyardError {
    return new CodeyardError("not-found", `not found: ${what}`);
  }

  static invalid(what: string): CodeyardError {
    return new CodeyardError("invalid", `invalid: ${what}`);
  }

  static other(message: string): CodeyardError {
    return new CodeyardError("other", message);
  }
}

/** A refused call. `permission` is the manifest line that would allow it, such as `net = ["api.example.com"]`. */
export class DeniedError extends CodeyardError {
  readonly permission: string;
  readonly why: Why;

  constructor(permission: string, why: Why) {
    const what = {
      "not-declared": "the manifest does not declare",
      "revoked-by-user": "the user turned off",
      "untrusted-workspace": "this folder is not trusted, which rules out",
      "revoked-extension": "this version was revoked, which rules out",
    }[why];
    super("denied", `denied: ${what} ${permission}\n   = fix: ${fixFor(permission, why)}`);
    this.name = "DeniedError";
    this.permission = permission;
    this.why = why;
  }

  /** What to do about it, in one line. */
  get fix(): string {
    return fixFor(this.permission, this.why);
  }
}

function fixFor(permission: string, why: Why): string {
  switch (why) {
    case "not-declared":
      return `add to [permissions]:  ${permission}`;
    case "revoked-by-user":
      return `the user turned off \`${permission}\` on the extension page; tell them what stops working without it`;
    case "untrusted-workspace":
      return "trust the folder, or set `untrusted = false` in the manifest if the extension is useless without trust";
    case "revoked-extension":
      return "the registry revoked this version: publish a fixed one";
  }
}

/** `types.error` from the host as an Error. A denial that names only the permission (`net`) gets its line from `line`. */
export function fromWit(e: WitError, line: () => string = () => ""): CodeyardError {
  switch (e.tag) {
    case "denied": {
      const permission = e.val.permission.includes("=") || !line() ? e.val.permission : line();
      return new DeniedError(permission, e.val.why);
    }
    case "not-found":
      return CodeyardError.notFound(e.val);
    case "cancelled":
      return new CodeyardError("cancelled", "cancelled");
    case "timeout":
      return new CodeyardError("timeout", "timed out");
    case "invalid":
      return CodeyardError.invalid(e.val);
    case "other":
      return CodeyardError.other(e.val);
  }
}

/** Any thrown value as `types.error`. A JavaScript error keeps its stack, so `cyx test` and the Inspector can map it. */
export function toWit(e: unknown): WitError {
  if (e instanceof DeniedError) return { tag: "denied", val: { permission: e.permission, why: e.why } };
  if (e instanceof CodeyardError) {
    const strip = (prefix: string) => (e.message.startsWith(prefix) ? e.message.slice(prefix.length) : e.message);
    switch (e.kind) {
      case "not-found":
        return { tag: "not-found", val: strip("not found: ") };
      case "cancelled":
        return { tag: "cancelled" };
      case "timeout":
        return { tag: "timeout" };
      case "invalid":
        return { tag: "invalid", val: strip("invalid: ") };
      default:
        return { tag: "other", val: describe(e) };
    }
  }
  return { tag: "other", val: describe(e) };
}

/** `message` plus the stack as `at fn (file:line:column)` lines, the same on every engine. */
export function describe(e: unknown): string {
  if (!(e instanceof Error)) return typeof e === "string" ? e : safeString(e);
  const head = e.name && e.name !== "Error" && e.name !== "CodeyardError" ? `${e.name}: ${e.message}` : e.message;
  const frames = stackFrames(e.stack ?? "");
  return frames.length ? `${head}\n${frames.map((f) => `    at ${f}`).join("\n")}` : head;
}

/** V8 (`    at fn (file:1:2)`) and SpiderMonkey (`fn@file:1:2`) stacks as `fn (file:1:2)`. */
export function stackFrames(stack: string): string[] {
  const out: string[] = [];
  for (const raw of stack.split("\n")) {
    const line = raw.trim();
    if (!line) continue;
    const v8 = /^at (?:(.+?) \()?(.+?:\d+:\d+)\)?$/.exec(line);
    if (v8) {
      out.push(v8[1] ? `${v8[1]} (${v8[2]})` : `<anonymous> (${v8[2]})`);
      continue;
    }
    const sm = /^(.*)@(.+?:\d+:\d+)$/.exec(line);
    if (sm) out.push(`${sm[1] || "<anonymous>"} (${sm[2]})`);
  }
  return out;
}

function safeString(v: unknown): string {
  try {
    return JSON.stringify(v) ?? String(v);
  } catch {
    return String(v);
  }
}
