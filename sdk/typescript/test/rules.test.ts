// The mock host's copy of Codeyard's rules (src/testing/rules.ts) against the Rust rules: rule-cases.json holds cases
// judged by crates/ext-manifest (`UPDATE_DOCS=1 cargo test -p ext-manifest reference` writes it).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { builtinLanguage, checkDiagnosticsTarget, checkStatusItem, checkUrl, EDITOR_OPEN_ARGUMENT, editorOpenTarget, globsAllow, ipAddressLine, LIMITS, outsideLine, pathMatches, relative, resultTooLarge, shownDiagnosticMessage } from "../src/testing/rules.ts";

const cases = JSON.parse(readFileSync(new URL("./rule-cases.json", import.meta.url), "utf8")) as {
  limits: Record<string, number>;
  globs: { glob: string; path: string; matches: boolean }[];
  allowed: { globs: string[]; path: string; allowed: boolean }[];
  paths: { folder: string; uri: string; relative: string | null }[];
  urls: ({ url: string; host: string; port: number } | { url: string; problem: string; message: string; host: string; denied: string | null })[];
  outside: { kind: string; path: string; line: string }[];
  diagnostics: {
    messages: { message: string; shown: string | null; cut?: boolean }[];
    targets: { declared: string[][] | null; language: string; reachable: boolean; otherFiles: number; error: string | null }[];
    languages: { path: string; language: string | null }[];
  };
  statusItems: { declared: string[] | null; id: string; text: string; tooltip: string | null; expect: { shown: string } | { error: string } }[];
  results: { bytes: number; error: string | null }[];
  editorOpen: { message: string; arguments: { arg: unknown; uri: string | null }[] };
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
    if ("port" in c) {
      assert.deepEqual(r, { host: c.host, port: c.port }, c.url);
      continue;
    }
    assert.deepEqual(r, { problem: c.problem, message: c.message, host: c.host }, c.url);
    // An IP address is a denial no entry can lift, with Codeyard's line.
    assert.equal(c.problem === "ip-literal" ? ipAddressLine(c.host) : null, c.denied, c.url);
  }
});

test("refusals no entry can lift read as in Codeyard", () => {
  for (const c of cases.outside) assert.equal(outsideLine(c.kind, c.path), c.line);
});

test("status items and answer sizes are checked as in Codeyard", () => {
  for (const c of cases.statusItems) {
    assert.deepEqual(checkStatusItem(c.declared ?? undefined, c.id, c.text, c.tooltip ?? undefined), c.expect, `${c.id} ${c.text}`);
  }
  for (const c of cases.results) assert.equal(resultTooLarge(c.bytes) ?? null, c.error, String(c.bytes));
});

test("diagnostics are checked as in Codeyard", () => {
  for (const c of cases.diagnostics.messages) {
    const r = shownDiagnosticMessage(c.message);
    assert.equal(r?.shown ?? null, c.shown, JSON.stringify(c.message));
    if (r) assert.equal(r.cut, c.cut, JSON.stringify(c.message));
  }
  for (const c of cases.diagnostics.targets) {
    assert.equal(checkDiagnosticsTarget(c.declared ?? undefined, c.language, c.reachable, c.otherFiles) ?? null, c.error, JSON.stringify(c));
  }
  for (const c of cases.diagnostics.languages) assert.equal(builtinLanguage(c.path) ?? null, c.language, c.path);
});

test("core:editor.open takes the arguments Codeyard takes", () => {
  assert.equal(EDITOR_OPEN_ARGUMENT, cases.editorOpen.message);
  for (const c of cases.editorOpen.arguments) assert.equal(editorOpenTarget(c.arg) ?? null, c.uri, JSON.stringify(c.arg));
});
