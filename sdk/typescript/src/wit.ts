// The `codeyard:api` WIT as JavaScript sees it through ComponentizeJS: records are objects with camelCase fields,
// variants are `{ tag, val }`, enums are kebab-case strings, `option<T>` is `T | undefined`, u64 is a BigInt and
// list<u8> a Uint8Array. The SDK speaks only these shapes to the host, so the component's imports and the mock host
// of `@codeyard/extension/testing` are interchangeable. Authors never see this module.

export type Ticket = bigint;

export type Why = "not-declared" | "revoked-by-user" | "untrusted-workspace" | "revoked-extension";

export type WitError =
  | { tag: "denied"; val: { permission: string; why: Why } }
  | { tag: "not-found"; val: string }
  | { tag: "cancelled" }
  | { tag: "timeout" }
  | { tag: "invalid"; val: string }
  | { tag: "other"; val: string };

export interface Position {
  line: number;
  character: number;
}

export interface Range {
  start: Position;
  end: Position;
}

export interface TextEdit {
  range: Range;
  newText: string;
}

export type Tone = "neutral" | "accent" | "success" | "warning" | "danger" | "muted";

export interface HttpRequest {
  method: string;
  url: string;
  headers: [string, string][];
  body: Uint8Array;
  timeoutMs: number | undefined;
}

export type DiagnosticSeverity = "error" | "warning" | "information" | "hint";

export interface Diagnostic {
  range: Range;
  severity: DiagnosticSeverity;
  message: string;
  code: string | undefined;
}

export interface Document {
  uri: string;
  language: string;
  version: number;
  text: string;
}

export interface WitFileStat {
  isDir: boolean;
  size: bigint;
  modified: bigint;
}

export interface WitFileResult {
  uri: string;
  contents: { tag: "ok"; val: Uint8Array } | { tag: "err"; val: WitError };
}

export interface Account {
  provider: string;
  label: string;
}

export type Completion =
  | { tag: "done" }
  | { tag: "failed"; val: WitError }
  | { tag: "bytes"; val: Uint8Array }
  | { tag: "text"; val: string | undefined }
  | { tag: "json"; val: string }
  | { tag: "chosen"; val: ArrayLike<number> }
  | { tag: "files"; val: WitFileResult[] }
  | { tag: "uris"; val: string[] }
  | { tag: "stat"; val: WitFileStat }
  | { tag: "document"; val: Document | undefined }
  | { tag: "ranges"; val: Range[] }
  | { tag: "exit"; val: number | undefined }
  | { tag: "account"; val: Account };

export type StreamEvent =
  | { tag: "http-head"; val: { status: number; headers: [string, string][] } }
  | { tag: "data"; val: Uint8Array }
  | { tag: "stdout"; val: Uint8Array }
  | { tag: "stderr"; val: Uint8Array }
  | { tag: "search-match"; val: { uri: string; range: Range; line: string } };

export interface FormatOptions {
  tabSize: number;
  insertSpaces: boolean;
}

export type Request =
  | { tag: "command"; val: [string, string] }
  | { tag: "render"; val: string }
  | { tag: "ui-event"; val: [string, string] }
  | { tag: "format"; val: [Document, Range | undefined, FormatOptions] }
  | { tag: "code-actions"; val: [Document, Range, Diagnostic[]] }
  | { tag: "run-code-action"; val: string }
  | { tag: "provide-tasks" }
  | { tag: "run-task"; val: string }
  | { tag: "settings-changed"; val: string[] }
  | { tag: "timer"; val: bigint }
  | { tag: "cancel-progress"; val: bigint }
  | { tag: "open-uri"; val: string }
  | { tag: "hover"; val: [Document, Position] }
  | { tag: "completion"; val: [Document, Position, string | undefined] }
  | { tag: "code-lens"; val: [Document, Range] }
  | { tag: "resolve-code-lens"; val: [Document, WitCodeLens] }
  | { tag: "inlay-hints"; val: [Document, Range] }
  | { tag: "scm-status"; val: [string, string] }
  | { tag: "scm-original"; val: [string, string] }
  | { tag: "scm-validate"; val: [string, string, string] }
  | { tag: "pull-requests"; val: PullQuery };

export type FileState = "modified" | "added" | "deleted" | "renamed" | "untracked" | "ignored" | "conflicted";

export interface WitScmAction {
  command: string;
  title: string;
  icon: string | undefined;
}

export interface WitScmState {
  head: string | undefined;
  input: { placeholder: string; value: string | undefined; accept: WitScmAction | undefined };
  groups: {
    id: string;
    label: string;
    resources: { uri: string; state: FileState; detail: string | undefined }[];
    resourceActions: WitScmAction[];
    groupActions: WitScmAction[];
    hideWhenEmpty: boolean;
  }[];
}

export interface WitCodeLens {
  id: string;
  range: Range;
  command: { title: string; command: string; argsJson: string } | undefined;
}

export interface WitInlayHint {
  position: Position;
  label: string;
  tooltip: string | undefined;
}

export interface WitCodeAction {
  id: string;
  title: string;
  edits: [string, TextEdit[]][];
  command: string | undefined;
  preferred: boolean;
  fixes: Diagnostic[];
}

export type Response =
  | { tag: "none" }
  | { tag: "json"; val: string }
  | { tag: "view"; val: string }
  | { tag: "edits"; val: TextEdit[] }
  | { tag: "code-actions"; val: WitCodeAction[] }
  | { tag: "tasks"; val: { id: string; label: string; detail: string | undefined; group: string }[] }
  | { tag: "ranges"; val: Range[] }
  | { tag: "hover"; val: WitHover | undefined }
  | { tag: "completions"; val: { items: WitCompletionItem[]; incomplete: boolean } }
  | { tag: "code-lenses"; val: WitCodeLens[] }
  | { tag: "inlay-hints"; val: WitInlayHint[] }
  | { tag: "scm"; val: WitScmState }
  | { tag: "text"; val: string | undefined }
  | { tag: "pull-requests"; val: PullAnswer };

// ---- `pull-requests` ----------------------------------------------------------------------------------------------

export interface PullRemote {
  provider: string;
  name: string;
  url: string;
  host: string;
  path: string;
  account: string | undefined;
}

export type PullState = "open" | "draft" | "merged" | "closed";

export interface PullSummary {
  id: string;
  reference: string;
  title: string;
  author: string;
  state: PullState;
  sourceBranch: string;
  targetBranch: string;
  updated: bigint;
  url: string | undefined;
}

export type PullCheckState = "pending" | "success" | "failure" | "neutral";

export interface PullCheck {
  name: string;
  state: PullCheckState;
  url: string | undefined;
}

export interface PullDetails {
  summary: PullSummary;
  description: string;
  checks: PullCheck[];
  labels: string[];
}

export type PullFileStatus = "added" | "modified" | "deleted" | "renamed";

export interface PullChangedFile {
  path: string;
  oldPath: string | undefined;
  status: PullFileStatus;
}

export interface PullFileContents {
  base: string | undefined;
  head: string | undefined;
}

export interface PullComment {
  author: string;
  body: string;
  created: bigint;
  path: string | undefined;
  line: number | undefined;
}

export interface PullCheckout {
  fetchUrl: string | undefined;
  ref: string;
  branch: string;
}

export type PullQuery =
  | { tag: "list"; val: PullRemote }
  | { tag: "details"; val: [PullRemote, string] }
  | { tag: "files"; val: [PullRemote, string] }
  | { tag: "contents"; val: [PullRemote, string, PullChangedFile] }
  | { tag: "comments"; val: [PullRemote, string] }
  | { tag: "add-comment"; val: [PullRemote, string, string] }
  | { tag: "checkout"; val: [PullRemote, string] };

export type PullAnswer =
  | { tag: "list"; val: PullSummary[] }
  | { tag: "details"; val: PullDetails }
  | { tag: "files"; val: PullChangedFile[] }
  | { tag: "contents"; val: PullFileContents }
  | { tag: "comments"; val: PullComment[] }
  | { tag: "checkout"; val: PullCheckout }
  | { tag: "done" };

export interface WitHover {
  contents: string;
  range: Range | undefined;
}

export type CompletionKind =
  | "text"
  | "method"
  | "function"
  | "constructor"
  | "field"
  | "variable"
  | "class"
  | "interface"
  | "module"
  | "property"
  | "unit"
  | "value"
  | "enum"
  | "keyword"
  | "snippet"
  | "color"
  | "file"
  | "reference"
  | "folder"
  | "enum-member"
  | "constant"
  | "struct"
  | "event"
  | "operator"
  | "type-parameter";

export interface WitCompletionItem {
  label: string;
  kind: CompletionKind;
  detail: string | undefined;
  documentation: string | undefined;
  insertText: string | undefined;
  snippet: boolean;
  range: Range | undefined;
  additionalEdits: TextEdit[];
  filterText: string | undefined;
  sortText: string | undefined;
}

export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";

export interface Environment {
  codeyardVersion: string;
  apiVersion: string;
  language: string;
  theme: "light" | "dark" | "high-contrast-light" | "high-contrast-dark";
  platform: "macos" | "linux" | "windows";
  remote: boolean;
  workspaceTrusted: boolean;
}

export type Severity = "info" | "warning" | "error";

export interface PickItem {
  label: string;
  detail: string | undefined;
  picked: boolean;
}

export interface PickOptions {
  title: string | undefined;
  placeholder: string | undefined;
  many: boolean;
}

export interface InputOptions {
  title: string | undefined;
  prompt: string | undefined;
  value: string | undefined;
  placeholder: string | undefined;
  password: boolean;
}

export interface StatusItem {
  id: string;
  text: string;
  tooltip: string | undefined;
  command: string | undefined;
  tone: Tone;
}

export type DecorationPlace = "gutter" | "line" | "after";

export interface Decoration {
  range: Range;
  place: DecorationPlace;
  tone: Tone;
  text: string | undefined;
  hover: string | undefined;
}

export interface ProcessCommand {
  program: string;
  args: string[];
  cwd: string | undefined;
  env: [string, string][];
  stdin: Uint8Array | undefined;
}

export interface SearchQuery {
  pattern: string;
  regex: boolean;
  caseSensitive: boolean;
  wholeWord: boolean;
  include: string[];
  max: number;
}

/// The imports, one object per interface (`import * as net from "codeyard:api/net@0.1.0"`). An interface the manifest
/// does not grant is left out of the component, so it is missing here; the SDK then refuses the call with the fix.
export interface Bindings {
  log: { log(level: LogLevel, message: string): void };
  env: { info(): Environment };
  tickets: { cancel(ticket: Ticket): void };
  requests: { respond(id: bigint, result: { tag: "ok"; val: Response } | { tag: "err"; val: WitError }): void };
  window: {
    showNotice(severity: Severity, message: string, actions: string[]): Ticket;
    quickPick(items: PickItem[], options: PickOptions): Ticket;
    inputBox(options: InputOptions): Ticket;
    startProgress(title: string, cancellable: boolean): bigint;
    reportProgress(id: bigint, message: string | undefined, percent: number | undefined): void;
    endProgress(id: bigint): void;
    /** Throws `{ payload: WitError }` when refused. */
    setStatusItem(item: StatusItem): void;
    removeStatusItem(id: string): void;
    openExternal(url: string): Ticket;
  };
  commands: { execute(command: string, argsJson: string): Ticket };
  storage: {
    get(key: string): Uint8Array | undefined;
    /** Throws `{ payload: WitError }` when the store would grow past its limit. */
    set(key: string, value: Uint8Array): void;
    delete(key: string): void;
    keys(): string[];
  };
  settings: { get(key: string): string | undefined };
  timers: { every(seconds: number): bigint; clear(id: bigint): void };
  ui: { invalidate(view: string): void };
  documents: {
    active(): Ticket;
    selections(): Ticket;
    read(uri: string): Ticket;
    applyEdits(uri: string, version: number, edits: TextEdit[]): Ticket;
    setSelections(uri: string, selections: Range[]): Ticket;
  };
  languages: {
    setDiagnostics(uri: string, diagnostics: Diagnostic[]): void;
    clearDiagnostics(uri: string | undefined): void;
    setDecorations(uri: string, set: string, decorations: Decoration[]): void;
  };
  workspace?: {
    folders(): string[];
    readFile(uri: string): Ticket;
    readMany(uris: string[]): Ticket;
    stat(uri: string): Ticket;
    findFiles(glob: string, max: number): Ticket;
    search(query: SearchQuery): Ticket;
  };
  workspaceWrite?: {
    writeFile(uri: string, contents: Uint8Array): Ticket;
    delete(uri: string): Ticket;
    rename(from: string, to: string): Ticket;
    createDirectory(uri: string): Ticket;
  };
  net?: { fetch(request: HttpRequest): Ticket };
  auth?: { session(provider: string, scopes: string[]): Ticket; fetch(provider: string, request: HttpRequest): Ticket };
  secrets?: { get(key: string): Ticket; set(key: string, value: string): Ticket; delete(key: string): Ticket };
  process?: { spawn(command: ProcessCommand): Ticket };
  clipboardRead?: { readText(): Ticket };
  clipboardWrite?: { writeText(text: string): Ticket };
  terminal?: { sendText(text: string): Ticket };
  scm: { refresh(provider: string): void };
}

/// The guest's exports (`interface guest`).
export interface Guest {
  activate(activation: { reason: string }): void;
  deactivate(): void;
  handle(id: bigint, request: Request): void;
  cancel(id: bigint): void;
  complete(ticket: Ticket, result: Completion): void;
  event(ticket: Ticket, event: StreamEvent): void;
}
