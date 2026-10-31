// The mock host's view check (src/testing/view-check.ts) against Codeyard's: view-cases.json holds views and events
// judged by crates/ext-ui (`cargo test -p ext-ui` keeps it current), and each must get the same verdict here, with the
// same problems.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { checkEvent, checkView } from "../src/testing/view-check.ts";

type Expect = "ok" | { malformed: string } | { tooLarge: number } | { problems: string[] };

const cases = JSON.parse(readFileSync(new URL("./view-cases.json", import.meta.url), "utf8")) as {
  views: { name: string; json: string; expect: Expect }[];
  events: { json: string; ok: boolean }[];
};

test("every view gets Codeyard's verdict", () => {
  assert.ok(cases.views.length > 300);
  for (const c of cases.views) {
    const got = checkView(c.json);
    if (c.expect === "ok") {
      assert.ok(got.ok, `${c.name}: refused (${got.ok ? "" : got.message}), Codeyard draws it`);
    } else if ("malformed" in c.expect) {
      assert.ok(!got.ok && got.reason === "malformed", `${c.name}: ${got.ok ? "drawn" : got.message}; Codeyard says: ${c.expect.malformed}`);
    } else if ("problems" in c.expect) {
      assert.ok(!got.ok, `${c.name}: drawn; Codeyard says: ${c.expect.problems.join("; ")}`);
      assert.deepEqual(got.problems, c.expect.problems, c.name);
    } else {
      assert.ok(!got.ok && got.reason === "too-large", c.name);
    }
  }
});

test("every event gets Codeyard's verdict", () => {
  for (const c of cases.events) assert.equal(checkEvent(c.json) === undefined, c.ok, c.json);
});

const node = (key: string | undefined, element: unknown, children: unknown[] = []) => ({ key, element, children });

test("the size, element, row and document caps are Codeyard's", () => {
  const rows = (n: number) => node("l", { list: { label: "L" } }, Array.from({ length: n }, (_, i) => node(`r${i}`, "divider")));
  assert.ok(checkView(JSON.stringify(rows(10_000))).ok);
  assert.deepEqual(checkView(JSON.stringify(rows(10_001))), {
    ok: false,
    reason: "invalid",
    message: "list: has 10001 rows; send at most 10000 at a time",
    problems: ["list: has 10001 rows; send at most 10000 at a time"],
  });
  const many = node(undefined, { column: {} }, Array.from({ length: 5 }, (_, c) => rows(10_000)).map((l, c) => ({ ...l, key: `c${c}` })));
  const verdict = checkView(JSON.stringify(many));
  assert.ok(!verdict.ok && verdict.message === "column: has 50006 elements; a view may have at most 50000", verdict.ok ? "" : verdict.message);
  const markdown = (source: string) => checkView(JSON.stringify(node(undefined, { markdown: { source } })));
  assert.ok(markdown("a".repeat(64 * 1024)).ok);
  assert.equal((markdown("é".repeat(32 * 1024 + 1)) as { message: string }).message, "markdown: the markdown is 65538 bytes; the limit is 65536");
  const big = checkView(" ".repeat(8 * 1024 * 1024 + 1));
  assert.ok(!big.ok && big.reason === "too-large");
  const deep = '{"element": {"column": {}}, "children": ['.repeat(500) + '{"element": "divider"}' + "]}".repeat(500);
  assert.ok(!checkView(deep).ok);
});
