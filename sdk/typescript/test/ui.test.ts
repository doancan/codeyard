// `ui` builds what Codeyard draws: every builder and option passes the mock host's check (Codeyard's rules, see
// view-check.test.ts) and writes the reference form of docs/extensions/reference/ui.md.

import assert from "node:assert/strict";
import { test } from "node:test";

import { ui, type ViewNode } from "../src/index.ts";
import { checkView } from "../src/testing/view-check.ts";

const drawn = (tree: ViewNode) => {
  const verdict = checkView(JSON.stringify(tree));
  assert.ok(verdict.ok, verdict.ok ? "" : verdict.message);
  return JSON.parse(JSON.stringify(tree));
};

test("every builder writes the form Codeyard reads", () => {
  const tree = ui.column(
    [
      ui.row([ui.text("plain"), ui.text("muted", { tone: "muted" })], { gap: 1 }),
      ui.heading("Title", 2),
      ui.icon("refresh", { tone: "accent", label: "Refresh" }),
      ui.image("https://example.com/a.png", "A"),
      ui.image(new Uint8Array([0x89, 0x50, 0x4e, 0x47]), ""),
      ui.button("go", "Go", { icon: "play", tone: "accent", disabled: true }),
      ui.textInput("name", "Name", { placeholder: "web", value: "api", multiline: true }),
      ui.checkbox("ok", "OK", { checked: true }),
      ui.select("pick", "Pick", [{ key: "a", label: "A" }, { key: "b", label: "B" }], { selected: "b" }),
      ui.list("files", "Files", [ui.text("a.txt", { key: "a" })], { selection: "many" }),
      ui.tree("projects", "Projects", [ui.tree("web", "web", [], { description: "3 builds", icon: "folder", more: "" })], { selection: "one", more: "p2" }),
      ui.table("builds", "Builds", ["Id", "State"], [ui.row([ui.text("b1"), ui.badge("ok", { tone: "success" })], { key: "b1" })]),
      ui.progress("Loading", { percent: 40 }),
      ui.markdown("# Hi"),
      ui.code("fn main() {}", { language: "rust" }),
      ui.keyValue({ Region: "eu" }),
      ui.banner("Careful", { tone: "warning" }),
      ui.link("docs", "Docs", { url: "https://example.com/docs" }),
      ui.divider(),
      ui.emptyState("Nothing", "Yet", [ui.button("add", "Add")], { icon: "inbox" }),
    ],
    { gap: 2, pad: 1 },
  );
  const json = drawn(tree);
  assert.deepEqual(json.element, { column: { gap: 2, pad: 1 } });
  assert.deepEqual(json.children[3], { element: { image: { source: { url: "https://example.com/a.png" }, alt: "A" } } });
  assert.deepEqual(json.children[4].element.image.source, { bytes: "iVBORw==" });
  assert.deepEqual(json.children[5], { key: "go", element: { button: { label: "Go", icon: "play", tone: "accent", disabled: true } } });
  assert.deepEqual(json.children[11].children[0], {
    key: "b1",
    element: { row: {} },
    children: [{ element: { text: { text: "b1" } } }, { element: { badge: { text: "ok", tone: "success" } } }],
  });
  assert.deepEqual(json.children[15], { element: { "key-value": { pairs: [{ name: "Region", value: "eu" }] } } });
  assert.deepEqual(json.children[18], { element: "divider" });
});

test("the template's tree is the reference form", () => {
  assert.deepEqual(drawn(ui.column([ui.heading("Clicks", 2), ui.text("2"), ui.button("add", "Add one")])), {
    element: { column: {} },
    children: [
      { element: { heading: { text: "Clicks", level: 2 } } },
      { element: { text: { text: "2" } } },
      { key: "add", element: { button: { label: "Add one" } } },
    ],
  });
});

test("image bytes are padded base64", () => {
  for (let len = 0; len < 8; len++) {
    const bytes = Uint8Array.from({ length: len }, (_, i) => i * 37 + 1);
    const node = ui.image(bytes, "") as { element: { image: { source: { bytes: string } } } };
    assert.deepEqual(new Uint8Array(Buffer.from(node.element.image.source.bytes, "base64")), bytes);
    assert.equal(node.element.image.source.bytes, Buffer.from(bytes).toString("base64"));
  }
});
