// The checks Codeyard makes on a view before drawing it (crates/ext-ui: `decode_json` and `validate`), and on the
// events it sends, so the mock host refuses what Codeyard refuses, with the same words. The format comes from
// ../view-schema.ts, generated from crates/ext-ui; test/view-cases.json holds views and events judged by ext-ui itself,
// and test/view-check.test.ts requires the same verdict and the same problems here.

import { VIEW_SCHEMA, type ViewNode } from "../view-schema.ts";

const L = VIEW_SCHEMA.limits as Record<
  | "viewBytes"
  | "nodes"
  | "depth"
  | "rowsPerPage"
  | "labelLen"
  | "textLen"
  | "documentBytes"
  | "keyLen"
  | "imageBytes"
  | "choices"
  | "columns"
  | "urlLen"
  | "iconLen"
  | "spaceMax"
  | "problems",
  number
>;

type Spec = (typeof VIEW_SCHEMA.elements)[number];
const SPECS = new Map<string, Spec>(VIEW_SCHEMA.elements.map((e) => [e.kind, e]));

/** What Codeyard makes of a view: drawn, or refused with the reason it shows in place of the view. */
export type ViewVerdict =
  | { ok: true; view: ViewNode }
  | { ok: false; reason: "too-large" | "malformed" | "invalid"; message: string; problems: string[] };

class Malformed extends Error {}

const utf8 = new TextEncoder();
const bytes = (s: string) => utf8.encode(s).length;
const chars = (s: string) => [...s].length;
const hex = (c: string) => c.codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0");
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

// ---- the JSON form (serde in ext-ui) -------------------------------------------------------------------------------

function fields(where: string, v: Record<string, unknown>, allowed: string[]) {
  for (const k of Object.keys(v)) {
    if (!allowed.includes(k)) throw new Malformed(`unknown field \`${k}\`${where}, expected one of ${allowed.map((a) => `\`${a}\``).join(", ")}`);
  }
}

const isU8 = (v: unknown) => typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= 255;

const BASE64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/** The number of bytes in canonical, padded base64 (the `STANDARD` engine), or undefined when it is not. */
function base64Length(s: string): number | undefined {
  if (!BASE64.test(s)) return undefined;
  const pad = s.endsWith("==") ? 2 : s.endsWith("=") ? 1 : 0;
  if (pad) {
    const last = B64.indexOf(s[s.length - pad - 1]!);
    if ((pad === 2 && last & 0x0f) || (pad === 1 && last & 0x03)) return undefined;
  }
  return (s.length / 4) * 3 - pad;
}

function property(kind: string, name: string, type: string, v: unknown) {
  const bad = (expected: string) => new Malformed(`invalid type for \`${kind}.${name}\`: ${JSON.stringify(v)}, expected ${expected}`);
  switch (type) {
    case "space":
    case "level":
    case "percent":
      if (!isU8(v)) throw bad("an integer from 0 to 255");
      return;
    case "tone":
      if (!VIEW_SCHEMA.tones.includes(v as string)) throw bad(`one of ${VIEW_SCHEMA.tones.join(", ")}`);
      return;
    case "selection":
      if (!VIEW_SCHEMA.selections.includes(v as string)) throw bad(`one of ${VIEW_SCHEMA.selections.join(", ")}`);
      return;
    case "bool":
      if (typeof v !== "boolean") throw bad("a boolean");
      return;
    case "choices":
    case "pairs": {
      const [a, b] = type === "choices" ? ["key", "label"] : ["name", "value"];
      if (!Array.isArray(v)) throw bad("a list");
      for (const item of v) {
        if (!isObject(item)) throw bad(`a list of {${a}, ${b}}`);
        fields(` in \`${kind}.${name}\``, item, [a!, b!]);
        if (typeof item[a!] !== "string" || typeof item[b!] !== "string") throw bad(`a list of {${a}, ${b}} with text`);
      }
      return;
    }
    case "columns":
      if (!Array.isArray(v) || v.some((c) => typeof c !== "string")) throw bad("a list of text");
      return;
    case "image": {
      if (!isObject(v) || Object.keys(v).length !== 1) throw bad('{"bytes": …} or {"url": …}');
      fields(` in \`${kind}.${name}\``, v, ["bytes", "url"]);
      const [which, text] = Object.entries(v)[0]!;
      if (typeof text !== "string") throw bad(`text in \`${which}\``);
      if (which === "bytes" && base64Length(text) === undefined) throw new Malformed(`the image is not base64`);
      return;
    }
    default:
      if (typeof v !== "string") throw bad("text");
  }
}

function element(e: unknown): void {
  if (e === "divider") return;
  if (typeof e === "string") {
    if (SPECS.has(e)) throw new Malformed(`\`"${e}"\` has properties: write {"${e}": {…}}`);
    throw new Malformed(`unknown element \`${e}\``);
  }
  if (!isObject(e)) throw new Malformed(`an element is {"<kind>": {properties}} or "divider", not ${JSON.stringify(e)}`);
  const kinds = Object.keys(e);
  if (kinds.length !== 1) throw new Malformed(`an element names one kind, not ${kinds.length}`);
  const kind = kinds[0]!;
  const spec = SPECS.get(kind);
  if (!spec) throw new Malformed(`unknown element \`${kind}\`, expected one of ${VIEW_SCHEMA.elements.map((s) => `\`${s.kind}\``).join(", ")}`);
  const props = e[kind];
  if (kind === "divider") {
    if (props !== null) throw new Malformed(`\`divider\` has no properties: write "divider"`);
    return;
  }
  if (!isObject(props)) throw new Malformed(`the properties of \`${kind}\` are an object, not ${JSON.stringify(props)}`);
  fields(` in \`${kind}\``, props, spec.properties.map((p) => p.name));
  for (const p of spec.properties) {
    const v = props[p.name];
    if (v === undefined) {
      if (p.required) throw new Malformed(`missing field \`${p.name}\` in \`${kind}\``);
    } else if (v === null) {
      if (!p.nullable) throw new Malformed(`\`${kind}.${p.name}\` may not be null`);
    } else {
      property(kind, p.name, p.type, v);
    }
  }
}

function shape(n: unknown, depth: number): void {
  // serde_json stops at 128 nested containers; each level of the tree is two (the node and its children).
  if (depth > 64) throw new Malformed("recursion limit exceeded");
  if (!isObject(n)) throw new Malformed(`a node is {"key", "element", "children"}, not ${JSON.stringify(n)?.slice(0, 40)}`);
  fields("", n, ["key", "element", "children"]);
  if (n.key !== undefined && n.key !== null && typeof n.key !== "string") throw new Malformed(`a key is text, not ${JSON.stringify(n.key)}`);
  if (!("element" in n)) throw new Malformed("missing field `element`");
  element(n.element);
  if (n.children !== undefined) {
    if (!Array.isArray(n.children)) throw new Malformed(`children are a list, not ${JSON.stringify(n.children)?.slice(0, 40)}`);
    for (const c of n.children) shape(c, depth + 1);
  }
}

// ---- the rules (validate.rs) ---------------------------------------------------------------------------------------

interface Node {
  key?: string | null;
  element: string | Record<string, Record<string, unknown> | null>;
  children?: Node[];
}

const kindOf = (n: Node) => (typeof n.element === "string" ? n.element : Object.keys(n.element)[0]!);
const propsOf = (n: Node): Record<string, unknown> => (typeof n.element === "string" ? {} : (n.element[kindOf(n)] ?? {}));
const keyOf = (n: Node) => (typeof n.key === "string" ? n.key : undefined);
const childrenOf = (n: Node) => n.children ?? [];

const isControl = (c: string) => {
  const p = c.codePointAt(0)!;
  return p < 0x20 || (p >= 0x7f && p <= 0x9f);
};

/** `ext_manifest::is_deceptive`: control characters and the invisible ones that can disguise text. */
const isDeceptive = (c: string) => {
  const p = c.codePointAt(0)!;
  return (
    isControl(c) ||
    (p >= 0x200b && p <= 0x200f) ||
    (p >= 0x202a && p <= 0x202e) ||
    (p >= 0x2060 && p <= 0x2064) ||
    (p >= 0x2066 && p <= 0x2069) ||
    p === 0x061c ||
    p === 0xfeff ||
    p === 0x00ad
  );
};

/** Unicode White_Space, as Rust's `char::is_whitespace`. */
const isWhitespace = (c: string) => {
  const p = c.codePointAt(0)!;
  return (p >= 9 && p <= 13) || p === 32 || p === 0x85 || p === 0xa0 || p === 0x1680 || (p >= 0x2000 && p <= 0x200a) || p === 0x2028 || p === 0x2029 || p === 0x202f || p === 0x205f || p === 0x3000;
};

/** Rust's `str::escape_debug`, as problems quote keys. */
function escapeDebug(s: string): string {
  let out = "";
  for (const c of s) {
    const named: Record<string, string> = { "\\": "\\\\", "'": "\\'", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t", "\0": "\\0" };
    if (named[c]) out += named[c];
    else if (isDeceptive(c)) out += `\\u{${c.codePointAt(0)!.toString(16)}}`;
    else out += c;
  }
  return out;
}

/** `ext_manifest::check_id_part`, for icon names. */
function idPart(s: string): string | undefined {
  if (s === "") return "is empty";
  if (bytes(s) > L.iconLen) return `is longer than ${L.iconLen} characters`;
  if (!/^[a-z]/.test(s)) return "must start with a lowercase letter";
  const c = [...s].find((c) => !/^[a-z0-9-]$/.test(c));
  if (c !== undefined) return `contains '${c}'; use lowercase a-z, 0-9 and -`;
  if (s.includes("--") || s.endsWith("-")) return "must not contain '--' or end with '-'";
  return undefined;
}

/** `HttpsUrl::parse`. */
function httpsUrl(s: string): string | undefined {
  if (bytes(s) > L.urlLen) return `is longer than ${L.urlLen} characters`;
  if (!s.startsWith("https://")) return "must start with https://";
  const c = [...s].find((c) => isWhitespace(c) || isDeceptive(c));
  if (c !== undefined) return `contains the character U+${hex(c)}`;
  const authority = s.slice(8).split(/[/?#]/)[0] ?? "";
  if (authority.includes("@")) return "must not contain a user name or password";
  const colon = authority.lastIndexOf(":");
  const host = colon >= 0 && /^[0-9]*$/.test(authority.slice(colon + 1)) ? authority.slice(0, colon) : authority;
  if (host === "" || !/^[A-Za-z0-9.-]+$/.test(host)) return "has no host name";
  return undefined;
}

class Validator {
  problems: string[] = [];
  at: string[] = [];

  problem(message: string) {
    if (this.problems.length < L.problems) this.problems.push(`${this.at.join(" > ")}: ${message}`);
  }

  node(n: Node, parent: string | undefined, depth: number) {
    if (this.problems.length >= L.problems) return;
    if (depth > L.depth) {
      this.problem(`is nested deeper than ${L.depth} levels`);
      return;
    }
    const kind = kindOf(n);
    const spec = SPECS.get(kind)!;
    const key = keyOf(n);
    if (key !== undefined) this.key("the key", key);
    else if (spec.interactive && !(kind === "tree" && parent === "tree")) this.problem(`a ${kind} needs a key`);
    this.element(kind, propsOf(n));
    this.children(n, kind);
    if (kind === "tree" && parent !== "tree") this.treeKeys(n);
    if (key !== undefined || parent === undefined) this.scopeKeys(n, new Set());
    childrenOf(n).forEach((child, i) => {
      const k = keyOf(child);
      this.at.push(k !== undefined ? `${kindOf(child)}[${escapeDebug(k)}]` : `${kindOf(child)}#${i}`);
      this.node(child, kind, depth + 1);
      this.at.pop();
    });
  }

  scopeKeys(n: Node, seen: Set<string>) {
    for (const child of childrenOf(n)) {
      const k = keyOf(child);
      if (k === undefined) this.scopeKeys(child, seen);
      else if (seen.has(k)) this.problem(`the key '${escapeDebug(k)}' is used twice; keys must be unique among siblings`);
      else seen.add(k);
    }
  }

  treeKeys(tree: Node) {
    const seen = new Set<string>();
    const stack = [...childrenOf(tree)];
    for (let item = stack.pop(); item !== undefined; item = stack.pop()) {
      if (kindOf(item) !== "tree") continue;
      const k = keyOf(item);
      if (k !== undefined) {
        if (seen.has(k)) this.problem(`the tree item key '${escapeDebug(k)}' is used twice; item keys must be unique in the whole tree`);
        else seen.add(k);
      }
      stack.push(...childrenOf(item));
    }
  }

  children(n: Node, kind: string) {
    const spec = SPECS.get(kind)!;
    const children = childrenOf(n);
    if (children.length > 0 && !spec.children) {
      this.problem(`a ${kind} has no children`);
      return;
    }
    if (spec.rows) {
      if (children.length > L.rowsPerPage) this.problem(`has ${children.length} rows; send at most ${L.rowsPerPage} at a time`);
      if (children.some((c) => keyOf(c) === undefined)) this.problem(`every row of a ${kind} needs a key`);
    }
    if (kind === "table") {
      const columns = (propsOf(n).columns as string[]).length;
      children.forEach((row, i) => {
        if (kindOf(row) !== "row") this.problem(`row ${i} is a ${kindOf(row)}; the rows of a table are rows`);
        else if (childrenOf(row).length !== columns) this.problem(`row ${i} has ${childrenOf(row).length} cells for ${columns} columns`);
      });
    } else if (kind === "tree") {
      const c = children.find((c) => kindOf(c) !== "tree");
      if (c) this.problem(`holds a ${kindOf(c)}; the items of a tree are trees`);
    } else if (kind === "empty-state") {
      const c = children.find((c) => kindOf(c) !== "button" && kindOf(c) !== "link");
      if (c) this.problem(`holds a ${kindOf(c)}; an empty-state takes only buttons and links`);
    }
  }

  element(kind: string, props: Record<string, unknown>) {
    let options = new Set<string>();
    for (const p of SPECS.get(kind)!.properties) {
      const v = props[p.name];
      if (v === undefined || v === null) continue;
      switch (p.type) {
        case "space":
          if ((v as number) > L.spaceMax) this.problem(`the ${p.what} is step ${v}; use 0 to ${L.spaceMax}`);
          break;
        case "label":
          this.label(p.what, v as string);
          break;
        case "alt":
          if (v !== "") this.label(p.what, v as string);
          break;
        case "body":
          this.body(p.what, v as string);
          break;
        case "document":
          this.document(p.what, v as string);
          break;
        case "level":
          if ((v as number) < 1 || (v as number) > 3) this.problem(`the level is ${v}; use 1, 2 or 3`);
          break;
        case "percent":
          if ((v as number) > 100) this.problem(`the percent is ${v}; use 0 to 100`);
          break;
        case "icon":
          this.icon(v as string);
          break;
        case "url":
          this.url(v as string);
          break;
        case "page":
          if (bytes(v as string) > L.keyLen) this.problem(`the page token is longer than ${L.keyLen} bytes`);
          break;
        case "choices": {
          const list = v as { key: string; label: string }[];
          if (list.length > L.choices) this.problem(`has ${list.length} options; the limit is ${L.choices}`);
          options = new Set();
          for (const o of list.slice(0, L.choices)) {
            this.key("an option key", o.key);
            this.label("an option label", o.label);
            if (options.has(o.key)) this.problem(`the option key '${escapeDebug(o.key)}' is used twice`);
            options.add(o.key);
          }
          break;
        }
        case "option-key":
          if (!options.has(v as string)) this.problem(`the selected option '${escapeDebug(v as string)}' is not one of the options`);
          break;
        case "pairs": {
          const list = v as { name: string; value: string }[];
          if (list.length > L.choices) this.problem(`has ${list.length} pairs; the limit is ${L.choices}`);
          for (const pair of list.slice(0, L.choices)) {
            this.label("a name", pair.name);
            this.body("a value", pair.value);
          }
          break;
        }
        case "columns": {
          const list = v as string[];
          if (list.length === 0 || list.length > L.columns) this.problem(`has ${list.length} columns; use 1 to ${L.columns}`);
          for (const c of list.slice(0, L.columns)) this.label("a column header", c);
          break;
        }
        case "image": {
          const source = v as { bytes?: string; url?: string };
          if (source.url !== undefined) {
            this.url(source.url);
          } else {
            const n = base64Length(source.bytes!)!;
            if (n === 0) this.problem("the image has no bytes");
            else if (n > L.imageBytes) this.problem(`the image is ${n} bytes; the limit is ${L.imageBytes}`);
          }
          break;
        }
      }
    }
  }

  label(what: string, s: string) {
    const c = [...s].find(isDeceptive);
    if ([...s].every(isWhitespace)) this.problem(`${what} is empty`);
    else if (chars(s) > L.labelLen) this.problem(`${what} is longer than ${L.labelLen} characters`);
    else if (c !== undefined) this.problem(`${what} contains the invisible or control character U+${hex(c)}`);
  }

  body(what: string, s: string) {
    if (chars(s) > L.textLen) this.problem(`${what} is longer than ${L.textLen} characters`);
    else this.controls(what, s);
  }

  document(what: string, s: string) {
    const n = bytes(s);
    if (n > L.documentBytes) this.problem(`${what} is ${n} bytes; the limit is ${L.documentBytes}`);
    else this.controls(what, s);
  }

  controls(what: string, s: string) {
    const c = [...s].find((c) => isControl(c) && c !== "\n" && c !== "\t" && c !== "\r");
    if (c !== undefined) this.problem(`${what} contains the control character U+${hex(c)}`);
  }

  key(what: string, k: string) {
    const c = [...k].find(isControl);
    if (k === "") this.problem(`${what} is empty`);
    else if (bytes(k) > L.keyLen) this.problem(`${what} is longer than ${L.keyLen} bytes`);
    else if (c !== undefined) this.problem(`${what} contains the control character U+${hex(c)}`);
  }

  icon(name: string) {
    const e = idPart(name);
    if (e) this.problem(`the icon name '${escapeDebug(name)}' ${e}`);
  }

  url(u: string) {
    const e = httpsUrl(u);
    if (e) this.problem(`the URL ${e}`);
  }
}

function count(view: Node): number {
  let n = 0;
  const stack = [view];
  for (let node = stack.pop(); node !== undefined; node = stack.pop()) {
    n++;
    stack.push(...childrenOf(node));
  }
  return n;
}

/** Check a view as Codeyard does when `render` answers with `json`: the size cap, the JSON form, then every rule. */
export function checkView(json: string): ViewVerdict {
  const size = bytes(json);
  if (size > L.viewBytes) {
    const message = `the view is ${size} bytes; the limit is ${L.viewBytes}`;
    return { ok: false, reason: "too-large", message, problems: [message] };
  }
  let value: unknown;
  try {
    value = JSON.parse(json);
    shape(value, 1);
  } catch (e) {
    const message = `the view is malformed: ${(e as Error).message}`;
    return { ok: false, reason: "malformed", message, problems: [message] };
  }
  const view = value as Node;
  const v = new Validator();
  v.at.push(kindOf(view));
  const n = count(view);
  if (n > L.nodes) v.problem(`has ${n} elements; a view may have at most ${L.nodes}`);
  else v.node(view, undefined, 1);
  if (v.problems.length) return { ok: false, reason: "invalid", message: v.problems.join("\n"), problems: v.problems };
  return { ok: true, view: value as ViewNode };
}

const isKeys = (v: unknown) => Array.isArray(v) && v.every((k) => typeof k === "string");

/** Why Codeyard could not send `json` as a `ui-event`, or undefined when it could. */
export function checkEvent(json: string): string | undefined {
  let e: unknown;
  try {
    e = JSON.parse(json);
  } catch (err) {
    return (err as Error).message;
  }
  const examples = '{"click": {"path": ["add"]}}';
  if (!isObject(e) || Object.keys(e).length !== 1) return `an event is one of ${Object.keys(VIEW_SCHEMA.events).join(", ")}, as ${examples}`;
  const [name, body] = Object.entries(e)[0]!;
  const fields = VIEW_SCHEMA.events[name];
  if (!fields) return `unknown event \`${name}\`, expected one of ${Object.keys(VIEW_SCHEMA.events).join(", ")}`;
  if (!isObject(body)) return `the ${name} event is {${fields.join(", ")}}, as ${examples}`;
  for (const f of fields) {
    const v = body[f];
    const ok =
      f === "path" || f === "rows"
        ? isKeys(v)
        : f === "item"
          ? v === undefined || v === null || typeof v === "string"
          : f === "value"
            ? isObject(v) &&
              Object.keys(v).length === 1 &&
              ((typeof v.text === "string") || typeof v.checked === "boolean" || typeof v.choice === "string")
            : typeof v === "string";
    if (!ok) return `the ${name} event's \`${f}\` is ${v === undefined ? "missing" : `not ${JSON.stringify(v)}`}`;
  }
  return undefined;
}
