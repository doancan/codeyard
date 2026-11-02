// Build a view's tree (docs/extensions/views.md) in the form Codeyard draws (docs/extensions/reference/ui.md):
//
//   import { ui, type Context } from "@codeyard/extension";
//
//   ctx.view("main", () => ui.column([ui.heading("Clicks", 2), ui.text("3"), ui.button("add", "Add one")], { gap: 2 }));
//
// Each function makes one node from what its element needs, then an optional object with the element's other
// properties and a `key`. Interactive elements (button, textInput, checkbox, select, list, table, tree, link) take
// their key first, since Codeyard needs one; every row of a list or table needs one too (`ui.text(name, { key: id })`).
// Containers take their children as a list. The result is a plain `ViewNode`:
// `{ key?, element: { <kind>: { properties } }, children? }`.

import type { ElementProps, Kind, Selection, Space, ViewNode } from "./view-schema.ts";

export type { ElementProps, Selection, Space, ViewNode };

/** The optional properties of `K` (all but `Needed`), and a key. */
type Options<K extends Kind, Needed extends keyof ElementProps[K] = never> = Omit<ElementProps[K], Needed> & { key?: string };

function node(kind: Kind, props: object, options: { key?: string } | undefined, children?: ViewNode[]): ViewNode {
  const { key, ...rest } = (options ?? {}) as { key?: string };
  const n: ViewNode = { element: (kind === "divider" ? "divider" : { [kind]: { ...props, ...rest } }) as ViewNode["element"] };
  if (key !== undefined) n.key = key;
  if (children?.length) n.children = children;
  return n;
}

/** Children top to bottom. */
export function column(children: ViewNode[] = [], options?: Options<"column">): ViewNode {
  return node("column", {}, options, children);
}

/** Children left to right; inside a table, one row (with a key and a child per column). */
export function row(children: ViewNode[] = [], options?: Options<"row">): ViewNode {
  return node("row", {}, options, children);
}

export function text(text: string, options?: Options<"text", "text">): ViewNode {
  return node("text", { text }, options);
}

export function heading(text: string, level: 1 | 2 | 3, options?: { key?: string }): ViewNode {
  return node("heading", { text, level }, options);
}

/** A product icon by name (`refresh`, `git-branch`). Without a `label` it is decorative. */
export function icon(icon: string, options?: Options<"icon", "icon">): ViewNode {
  return node("icon", { icon }, options);
}

/**
 * An image: an `https:` URL Codeyard fetches through the `net` permission, or the bytes of a PNG, JPEG, GIF, WebP or
 * SVG of at most 1 MB. An empty `alt` marks it decorative.
 */
export function image(source: string | Uint8Array, alt: string, options?: { key?: string }): ViewNode {
  const src = typeof source === "string" ? { url: source } : { bytes: base64(source) };
  return node("image", { source: src, alt }, options);
}

/** Sends `{ click: { path: [..., key] } }`. */
export function button(key: string, label: string, options?: Options<"button", "label">): ViewNode {
  return node("button", { label }, { ...options, key });
}

/** Sends `change` when the user pauses typing, and `submit` on Enter in a single-line input. */
export function textInput(key: string, label: string, options?: Options<"text-input", "label">): ViewNode {
  return node("text-input", { label }, { ...options, key });
}

export function checkbox(key: string, label: string, options?: Options<"checkbox", "label">): ViewNode {
  return node("checkbox", { label }, { ...options, key });
}

/** `options`: at most 256, each with a key and a label. */
export function select(key: string, label: string, options: { key: string; label: string }[], more?: Options<"select", "label" | "options">): ViewNode {
  return node("select", { label, options }, { ...more, key });
}

/** Each row has a key; at most 10,000. Sends `select` when the selection changes. */
export function list(key: string, label: string, rows: ViewNode[] = [], options?: Options<"list", "label">): ViewNode {
  return node("list", { label }, { ...options, key }, rows);
}

/** A tree, or one of its items: its children are its items, which are trees too. */
export function tree(key: string, label: string, items: ViewNode[] = [], options?: Options<"tree", "label">): ViewNode {
  return node("tree", { label }, { ...options, key }, items);
}

/** Each row is a `ui.row` with a key and one child per column. */
export function table(key: string, label: string, columns: string[], rows: ViewNode[] = [], options?: Options<"table", "label" | "columns">): ViewNode {
  return node("table", { label, columns }, { ...options, key }, rows);
}

export function badge(text: string, options?: Options<"badge", "text">): ViewNode {
  return node("badge", { text }, options);
}

/** `percent` 0 to 100; leave it out while the end is unknown. */
export function progress(label: string, options?: Options<"progress", "label">): ViewNode {
  return node("progress", { label }, options);
}

/** CommonMark, drawn natively. Remote images are not loaded. */
export function markdown(source: string, options?: { key?: string }): ViewNode {
  return node("markdown", { source }, options);
}

export function code(source: string, options?: Options<"code", "source">): ViewNode {
  return node("code", { source }, options);
}

/** Name and value lines, at most 256: a list of pairs, or an object. */
export function keyValue(pairs: { name: string; value: string }[] | Record<string, string>, options?: { key?: string }): ViewNode {
  const list = Array.isArray(pairs) ? pairs : Object.entries(pairs).map(([name, value]) => ({ name, value }));
  return node("key-value", { pairs: list }, options);
}

export function banner(text: string, options?: Options<"banner", "text">): ViewNode {
  return node("banner", { text }, options);
}

/** With a `url`, Codeyard opens it after the user agrees; without one, the extension gets a `link` event. */
export function link(key: string, label: string, options?: Options<"link", "label">): ViewNode {
  return node("link", { label }, { ...options, key });
}

export function divider(options?: { key?: string }): ViewNode {
  return node("divider", {}, options);
}

/** Shown when a view has nothing yet; its children are its actions, buttons and links. */
export function emptyState(title: string, text: string, actions: ViewNode[] = [], options?: Options<"empty-state", "title" | "text">): ViewNode {
  return node("empty-state", { title, text }, options, actions);
}

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/** Standard base64 with padding, as Codeyard reads image bytes (no `Buffer` or `btoa` inside a component). */
function base64(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const n = (bytes[i]! << 16) | ((bytes[i + 1] ?? 0) << 8) | (bytes[i + 2] ?? 0);
    const left = bytes.length - i;
    out += B64[(n >> 18) & 63]! + B64[(n >> 12) & 63]! + (left > 1 ? B64[(n >> 6) & 63]! : "=") + (left > 2 ? B64[n & 63]! : "=");
  }
  return out;
}
