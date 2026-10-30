// The API's values as authors use them. Same field names as the WIT (camelCase), plain numbers instead of BigInt, and
// optional fields that may be left out.

import type {
  WitCodeLens,
  WitInlayHint,
  DecorationPlace,
  Diagnostic as WitDiagnostic,
  DiagnosticSeverity,
  Document,
  FormatOptions,
  LogLevel,
  Position,
  Range,
  Severity,
  TextEdit,
  Tone,
  Why,
} from "./wit.ts";

export type { DecorationPlace, DiagnosticSeverity, Document, FormatOptions, LogLevel, Position, Range, Severity, TextEdit, Tone, Why };

/** A file or folder. Opaque: the same code works on local and SSH folders. */
export type Uri = string;

/** One line, from column `start` to `end`. */
export function lineRange(line: number, start: number, end: number): Range {
  return { start: { line, character: start }, end: { line, character: end } };
}

export interface Diagnostic {
  range: Range;
  severity: DiagnosticSeverity;
  message: string;
  code?: string;
}

export function fromWitDiagnostic(d: WitDiagnostic): Diagnostic {
  return d.code === undefined ? { range: d.range, severity: d.severity, message: d.message } : { ...d, code: d.code };
}

export function toWitDiagnostic(d: Diagnostic): WitDiagnostic {
  return { range: d.range, severity: d.severity, message: d.message, code: d.code };
}

export interface FileStat {
  isDir: boolean;
  size: number;
  /** Unix milliseconds. */
  modified: number;
}

export interface Account {
  provider: string;
  /** What the user sees, such as the account name. */
  label: string;
}

/** What the extension runs in (`env.info`). */
export interface Env {
  codeyardVersion: string;
  apiVersion: string;
  /** The interface language, such as `en` or `tr`. */
  language: string;
  theme: "light" | "dark" | "high-contrast-light" | "high-contrast-dark";
  platform: "macos" | "linux" | "windows";
  /** The workspace is on an SSH host. */
  remote: boolean;
  workspaceTrusted: boolean;
}

/** A quick pick entry: its label, or the label with a detail line. */
export type PickItem = string | { label: string; detail?: string; picked?: boolean };

export interface PickOptions {
  title?: string;
  placeholder?: string;
}

export interface InputOptions {
  title?: string;
  prompt?: string;
  value?: string;
  placeholder?: string;
  password?: boolean;
}

export interface StatusItem {
  /** Local to the extension, as in `[[status-bar]]`. */
  id: string;
  text: string;
  tooltip?: string;
  /** A command id to run on click. */
  command?: string;
  tone?: Tone;
}

export interface Decoration {
  range: Range;
  place: DecorationPlace;
  tone?: Tone;
  text?: string;
  hover?: string;
}

/**
 * A code lens: clickable text shown with its line that runs a command with arguments. A lens without `command` is
 * resolved once it is on screen (`ctx.resolveCodeLens`).
 */
export interface CodeLens {
  /** Passed back to `resolveCodeLens`. */
  id?: string;
  /** The lens belongs to the range's first line. */
  range: Range;
  command?: LensCommand;
}

/** What a code lens shows and runs. */
export interface LensCommand {
  title: string;
  /** The extension's own command (local or full id), a dependency's full id, or a `core:` command the manifest lists. */
  command: string;
  /** The command's argument, as its handler receives it. */
  args?: unknown;
}

/** An inlay hint: short text at the end of its line, such as a type. */
export interface InlayHint {
  position: Position;
  label: string;
  tooltip?: string;
}

export function toWitCodeLens(l: CodeLens): WitCodeLens {
  const command = l.command && { title: l.command.title, command: l.command.command, argsJson: JSON.stringify(l.command.args ?? null) };
  return { id: l.id ?? "", range: l.range, command };
}

export function fromWitCodeLens(l: WitCodeLens): CodeLens {
  const lens: CodeLens = { id: l.id, range: l.range };
  if (l.command) lens.command = { title: l.command.title, command: l.command.command, args: JSON.parse(l.command.argsJson || "null") };
  return lens;
}

export function toWitInlayHint(h: InlayHint): WitInlayHint {
  return { position: h.position, label: h.label, tooltip: h.tooltip };
}

/** A code action a provider offers. */
export interface CodeAction {
  /** Passed back to `onCodeAction` when chosen and `command` is set. */
  id: string;
  title: string;
  /** Edits per document uri, applied when chosen. */
  edits?: Record<Uri, TextEdit[]>;
  command?: string;
  preferred?: boolean;
  /** The diagnostics this action fixes. */
  fixes?: Diagnostic[];
}

/**
 * A view's element tree (docs/extensions/reference/ui.md): `{ type: "column", children: [...] }`, `{ type: "text",
 * text: "…" }`, `{ type: "button", key: "add", label: "Add one" }` and the other built-in elements.
 */
export interface Element {
  type: string;
  key?: string;
  children?: Element[];
  [field: string]: unknown;
}

/** A program to run (`run = [...]`). */
export interface Command {
  /** A name from `run`, or a path inside the package. */
  program: string;
  args?: string[];
  /** Defaults to the first workspace folder. */
  cwd?: Uri;
  /** Added to a minimal environment; the user's environment is not passed. */
  env?: Record<string, string>;
  stdin?: string | Uint8Array;
}

export interface Output {
  /** None when killed by a signal. */
  code: number | undefined;
  stdout: Uint8Array;
  stderr: Uint8Array;
}

export type Chunk = { stdout: Uint8Array } | { stderr: Uint8Array };

export interface SearchMatch {
  uri: Uri;
  range: Range;
  /** The matching line, for display. */
  line: string;
}

export interface SearchOptions {
  regex?: boolean;
  caseSensitive?: boolean;
  wholeWord?: boolean;
  /** Only files matching these globs; empty means all the extension may read. */
  include?: string[];
  max?: number;
}
