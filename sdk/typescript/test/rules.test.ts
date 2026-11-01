// The mock host's copy of Codeyard's rules (src/testing/rules.ts) against the Rust rules: rule-cases.json holds cases
// judged by crates/ext-manifest (`UPDATE_DOCS=1 cargo test -p ext-manifest reference` writes it).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { checkStatusItem, checkUrl, globsAllow, LIMITS, pathMatches, relative, resultTooLarge } from "../src/testing/rules.ts";

const cases = JSON.parse(readFileSync(new URL("./rule-cases.json", import.meta.url), "utf8")) as {
  limits: Record<string, number>;
  globs: { glob: string; path: string; matches: boolean }[];
  allowed: { globs: string[]; path: string; allowed: boolean }[];
  paths: { folder: string; uri: string; relative: string | null }[];
  urls: ({ url: string; host: string; port: number } | { url: string; problem: string; message: string })[];
  statusItems: { declared: string[] | null; id: string; text: string; tooltip: string | null; expect: { shown: string } | { error: string } }[];
  results: { bytes: number; error: string | null }[];
};

test("the limits are Codeyard's", () => {
  assert.deepEqual({ ...LIMITS }, cases.limits);
});

test("globs match whole paths, as in Codeyard", () => {
  for (const c of cases.globs) assert.equal(pathMatches(c.glob, c.path), c.matches, `${c.glob} vs ${c.path}`);
  for (const c of cases.allowed) assert.equal(globsAllow(c.globs, c.path), c.allowed, `${c.globs} vs ${c.path}`);
});

test("paths are resolved inside the folder, as in Codeyard", () => {
  for (const c of cases.paths) assert.equal(relative([c.folder], c.uri) ?? null, c.relative, c.uri);
});

test("request URLs are checked as in Codeyard", () => {
  for (const c of cases.urls) {
    const r = checkUrl(c.url);
    if ("host" in c) assert.deepEqual(r, { host: c.host, port: c.port }, c.url);
    else assert.deepEqual(r, { problem: c.problem, message: c.message }, c.url);
  }
});

test("status items and answer sizes are checked as in Codeyard", () => {
  for (const c of cases.statusItems) {
    assert.deepEqual(checkStatusItem(c.declared ?? undefined, c.id, c.text, c.tooltip ?? undefined), c.expect, `${c.id} ${c.text}`);
  }
  for (const c of cases.results) assert.equal(resultTooLarge(c.bytes) ?? null, c.error, String(c.bytes));
});
