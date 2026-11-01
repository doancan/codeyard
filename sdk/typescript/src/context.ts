// `Context`: everything an extension registers and every call it makes. Calls that reach the app return Promises; the
// rest (storage, settings, log, diagnostics, status items) return at once.

import { CodeyardError, fromWit } from "./errors.ts";
import { fetch, hostOf, type FetchInit, type FetchResponse } from "./fetch.ts";
import { handlers } from "./handlers.ts";
import { registerPullRequests, type PullRequestProvider } from "./pulls.ts";
import { background, call, host, optional, wait } from "./runtime.ts";
import {
  fromWitDiagnostic,
  toWitDiagnostic,
  type Account,
  type Chunk,
  type CodeAction,
  type CodeLens,
  type InlayHint,
  type Command,
  type CompletionItem,
  type CompletionList,
  type Decoration,
  type Diagnostic,
  type Document,
  type Env,
  type FileStat,
  type FormatOptions,
  type Hover,
  type InputOptions,
  type LogLevel,
  type Output,
  type PickItem,
  type PickOptions,
  type Position,
  type Range,
  type SearchMatch,
  type SearchOptions,
  type Severity,
  type StatusItem,
  type TextEdit,
  type Uri,
  type ViewEvent,
  type ViewNode,
} from "./types.ts";
import type { ScmState } from "./scm.ts";
import type { Completion, StreamEvent } from "./wit.ts";

type Awaitable<T> = T | Promise<T>;

function unexpected(c: Completion): CodeyardError {
  return CodeyardError.other(`the host answered with something unexpected: ${c.tag}`);
}

/** Wait for a ticket; a refusal gets its manifest line from `line`. */
async function result(p: Promise<Completion>, line: () => string = () => ""): Promise<Completion> {
  const c = await p;
  if (c.tag === "failed") throw fromWit(c.val, line);
  return c;
}

/** `read = ["**\/vercel.json"]` for a file uri: the narrowest glob the SDK can name without knowing the workspace. */
function pathLine(permission: string, uri: string): string {
  const name = uri.slice(uri.lastIndexOf("/") + 1) || uri;
  return `${permission} = ["**/${name}"]`;
}

const utf8 = (s: string | Uint8Array): Uint8Array => (typeof s === "string" ? new TextEncoder().encode(s) : s);

/** The extension's handle on Codeyard. One per extension; capture it in closures freely. */
export class Context {
  /** The extension's persisted key-value store (50 MiB in all). */
  readonly storage: Storage = new Storage();

  /**
   * In-memory values that survive a hot reload in `cyx dev`: the SDK saves them when the old build stops and gives them
   * back to the new one. Any other start begins empty. Use `storage` for what must outlast the session.
   */
  readonly state: State = new State();

  // ---- registration ------------------------------------------------------------------------------------------------

  /**
   * Run `f` for the manifest's command `id` (its local id, like `refresh`). The arguments are JSON (`null` when none);
   * whatever `f` returns is sent back as JSON.
   *
   * ```ts
   * ctx.command("greet", async (ctx) => {
   *   ctx.notice("Hello!");
   * });
   * ```
   */
  command(id: string, f: (ctx: Context, args: any) => Awaitable<unknown>): void {
    handlers.commands.set(id, f);
  }

  /**
   * Draw the manifest's view `id`: `f` returns its tree, built with `ui` or written in the form of
   * docs/extensions/reference/ui.md. The view is drawn again after `invalidate(id)`.
   */
  view(id: string, f: (ctx: Context) => Awaitable<ViewNode>): void {
    handlers.views.set(id, f);
  }

  /** Handle what happens in view `id` (a click, a changed input): the event as Codeyard sends it, `{ click: { path: ["add"] } }`. */
  onViewEvent(id: string, f: (ctx: Context, event: ViewEvent) => Awaitable<void>): void {
    handlers.viewEvents.set(id, f);
  }

  /** Format documents: return the edits for the document, or for `range` of it. */
  formatter(f: (ctx: Context, document: Document, range: Range | undefined, options: FormatOptions) => Awaitable<TextEdit[]>): void {
    handlers.formatter = f;
  }

  /** Offer code actions for a range and the diagnostics on it. */
  codeActions(f: (ctx: Context, document: Document, range: Range, diagnostics: Diagnostic[]) => Awaitable<CodeAction[]>): void {
    handlers.codeActions = f;
  }

  /** The user chose a code action that has a `command`: its `id`. */
  onCodeAction(f: (ctx: Context, id: string) => Awaitable<void>): void {
    handlers.runCodeAction = f;
  }

  /**
   * Describe what is at a position of a document of the manifest's `[[hover]]` languages: shown in the hover popup
   * next to the language server's, under the extension's name. Answer within the provider deadline.
   */
  hover(f: (ctx: Context, document: Document, position: Position) => Awaitable<Hover | undefined>): void {
    handlers.hover = f;
  }

  /**
   * Offer completions at a position of a document of the manifest's `[[completion]]` languages: they join the
   * language server's in the list. `trigger` is the character typed when one of the manifest's `trigger-characters`
   * asked, undefined when the user typed a word or asked. Answer within the provider deadline: a late answer is dropped.
   */
  completion(
    f: (ctx: Context, document: Document, position: Position, trigger: string | undefined) => Awaitable<CompletionItem[] | CompletionList>,
  ): void {
    handlers.completion = f;
  }

  /**
   * Code lenses for the lines of `document` in view: `range` starts at the first and ends at the start of the line
   * after the last. Asked again after an edit (once typing pauses) and after a scroll; a slow answer is dropped.
   */
  codeLenses(f: (ctx: Context, document: Document, range: Range) => Awaitable<CodeLens[]>): void {
    handlers.codeLenses = f;
  }

  /** Give a lens returned without `command` its command, once it is on screen. */
  resolveCodeLens(f: (ctx: Context, document: Document, lens: CodeLens) => Awaitable<CodeLens>): void {
    handlers.resolveCodeLens = f;
  }

  /** Inlay hints for the lines of `document` in view (`range`, as for `codeLenses`): short text at the end of a line. */
  inlayHints(f: (ctx: Context, document: Document, range: Range) => Awaitable<InlayHint[]>): void {
    handlers.inlayHints = f;
  }

  /** A `codeyard://ext/<id>/…` link was opened: its path and query. */
  onUri(f: (ctx: Context, uri: string) => Awaitable<void>): void {
    handlers.openUri = f;
  }

  /**
   * Serve the manifest's `[[pull-requests]]` provider `id` (its local id): list a remote's pull (merge) requests and
   * show one. Call the forge with `ctx.fetchAs(remote.account, …)`, so the extension never sees the token.
   */
  pullRequests(id: string, provider: PullRequestProvider): void {
    registerPullRequests(id, provider);
  }

  /** The extension's settings changed: their keys. */
  onSettingsChanged(f: (ctx: Context, keys: string[]) => Awaitable<void>): void {
    handlers.settingsChanged.push(f);
  }

  /** Run `f` every `seconds` (at least 30), paused while the window is unfocused. An active timer keeps the extension running. */
  every(seconds: number, f: (ctx: Context) => Awaitable<void>): Timer {
    const id = host().timers.every(Math.max(0, Math.floor(seconds)));
    handlers.timers.set(id, f);
    return new Timer(id);
  }

  /** Run work in the background, such as work started from `activate`. A failure is logged with its stack. */
  background(work: Promise<unknown> | (() => unknown)): void {
    background(work);
  }

  /** Why the extension started: `command:<id>`, `view:<id>`, `workspace-contains`, … */
  get activationReason(): string {
    return handlers.reason;
  }

  // ---- free calls --------------------------------------------------------------------------------------------------

  log(level: LogLevel, message: string): void {
    host().log.log(level, message);
  }

  env(): Env {
    return host().env.info();
  }

  /** A setting declared in the manifest; undefined when unset and without a default. */
  setting<T = unknown>(key: string): T | undefined {
    const json = host().settings.get(key);
    if (json === undefined) return undefined;
    try {
      return JSON.parse(json) as T;
    } catch (e) {
      throw CodeyardError.invalid(`setting ${key}: ${(e as Error).message}`);
    }
  }

  /** Ask for view `id` to be drawn again. */
  invalidate(view: string): void {
    host().ui.invalidate(view);
  }

  // ---- source control (`[[scm]]`) ----------------------------------------------------------------------------------

  /**
   * The state of a folder that `[[scm]]` provider `id` claims: its groups of changed files and its input box. Asked
   * when Source Control shows the folder, after files change, and after `scmRefresh(id)`.
   */
  scmStatus(id: string, f: (ctx: Context, root: string) => Awaitable<ScmState>): void {
    handlers.scmStatus.set(id, f);
  }

  /** The text of a file as provider `id` last recorded it, for the gutter's change marks; undefined when untracked. */
  scmOriginal(id: string, f: (ctx: Context, uri: string) => Awaitable<string | undefined>): void {
    handlers.scmOriginal.set(id, f);
  }

  /** Check the input box's text: a message to show under the box, or undefined when it is fine. */
  scmValidate(id: string, f: (ctx: Context, root: string, text: string) => Awaitable<string | undefined>): void {
    handlers.scmValidate.set(id, f);
  }

  /** Ask Codeyard to read provider `id`'s state again now (after the extension changed something itself). */
  scmRefresh(id: string): void {
    host().scm.refresh(id);
  }

  // ---- window ------------------------------------------------------------------------------------------------------

  /** An information notice; does not wait for it to be dismissed. */
  notice(message: string): void {
    background(call(() => host().window.showNotice("info", message, [])));
  }

  /** A notice with buttons: the index of the one clicked, or undefined when dismissed. */
  async notify(severity: Severity, message: string, actions: string[] = []): Promise<number | undefined> {
    const c = await result(call(() => host().window.showNotice(severity, message, actions)));
    if (c.tag === "chosen") return c.val[0];
    if (c.tag === "done") return undefined;
    throw unexpected(c);
  }

  /** One item from a list: its index, or undefined when dismissed. */
  async quickPick(items: PickItem[], options: PickOptions = {}): Promise<number | undefined> {
    return (await this.#pick(items, options, false))[0];
  }

  /** Any number of items: their indices; empty when dismissed. */
  async quickPickMany(items: PickItem[], options: PickOptions = {}): Promise<number[]> {
    return this.#pick(items, options, true);
  }

  async #pick(items: PickItem[], options: PickOptions, many: boolean): Promise<number[]> {
    const list = items.map((i) => (typeof i === "string" ? { label: i, detail: undefined, picked: false } : { label: i.label, detail: i.detail, picked: i.picked ?? false }));
    const c = await result(call(() => host().window.quickPick(list, { title: options.title, placeholder: options.placeholder, many })));
    if (c.tag === "chosen") return Array.from(c.val);
    throw unexpected(c);
  }

  /** What the user typed, or undefined when dismissed. */
  async input(options: InputOptions = {}): Promise<string | undefined> {
    const o = { title: options.title, prompt: options.prompt, value: options.value, placeholder: options.placeholder, password: options.password ?? false };
    const c = await result(call(() => host().window.inputBox(o)));
    if (c.tag === "text") return c.val;
    throw unexpected(c);
  }

  /** Show progress until `end()`. Prefer `withProgress`, which ends it for you. */
  progress(title: string, cancellable = false): Progress {
    return new Progress(host().window.startProgress(title, cancellable));
  }

  /** Show progress while `work` runs. */
  async withProgress<T>(title: string, work: (progress: Progress) => Awaitable<T>, cancellable = false): Promise<T> {
    const p = this.progress(title, cancellable);
    try {
      return await work(p);
    } finally {
      p.end();
    }
  }

  setStatusItem(item: StatusItem): void {
    try {
      host().window.setStatusItem({ id: item.id, text: item.text, tooltip: item.tooltip, command: item.command, tone: item.tone ?? "neutral" });
    } catch (e) {
      throw thrown(e);
    }
  }

  removeStatusItem(id: string): void {
    host().window.removeStatusItem(id);
  }

  /** Open an `https:` URL in the browser; only while handling a user action. */
  async openExternal(url: string): Promise<void> {
    await result(call(() => host().window.openExternal(url)));
  }

  /** Run a command: the extension's own, a dependency's, or a core one the manifest lists. */
  async execute(command: string, args: unknown = null): Promise<unknown> {
    const c = await result(
      call(() => host().commands.execute(command, JSON.stringify(args ?? null))),
      () => `commands = ["${command}"]`,
    );
    if (c.tag === "json") return c.val.trim() ? JSON.parse(c.val) : null;
    if (c.tag === "done") return null;
    throw unexpected(c);
  }

  // ---- documents and languages -------------------------------------------------------------------------------------

  /** The active editor's document. A command the user started may read it without `read`. */
  async activeDocument(): Promise<Document | undefined> {
    const c = await result(call(() => host().documents.active()), () => `read = ["<glob>"]`);
    if (c.tag === "document") return c.val;
    throw unexpected(c);
  }

  async selections(): Promise<Range[]> {
    const c = await result(call(() => host().documents.selections()), () => `read = ["<glob>"]`);
    if (c.tag === "ranges") return c.val;
    throw unexpected(c);
  }

  /** An open document, or undefined when it is not open. */
  async readDocument(uri: Uri): Promise<Document | undefined> {
    const c = await result(call(() => host().documents.read(uri)), () => pathLine("read", uri));
    if (c.tag === "document") return c.val;
    throw unexpected(c);
  }

  /** Apply edits made against `version`; rejects with `invalid` when the document changed since. */
  async applyEdits(uri: Uri, version: number, edits: TextEdit[]): Promise<void> {
    await result(call(() => host().documents.applyEdits(uri, version, edits)), () => pathLine("write", uri));
  }

  async setSelections(uri: Uri, selections: Range[]): Promise<void> {
    await result(call(() => host().documents.setSelections(uri, selections)));
  }

  /** Replace this extension's diagnostics for `uri`. */
  setDiagnostics(uri: Uri, diagnostics: Diagnostic[]): void {
    host().languages.setDiagnostics(uri, diagnostics.map(toWitDiagnostic));
  }

  /** Remove this extension's diagnostics for `uri`, or everywhere. */
  clearDiagnostics(uri?: Uri): void {
    host().languages.clearDiagnostics(uri);
  }

  /** Replace the named set of decorations on `uri`. */
  setDecorations(uri: Uri, set: string, decorations: Decoration[]): void {
    const list = decorations.map((d) => ({ range: d.range, place: d.place, tone: d.tone ?? "neutral", text: d.text, hover: d.hover }));
    host().languages.setDecorations(uri, set, list);
  }

  // ---- workspace (`read`, `write`) ---------------------------------------------------------------------------------

  /** The workspace's folders; empty without `read`. */
  folders(): Uri[] {
    return host().workspace?.folders() ?? [];
  }

  async readFile(uri: Uri): Promise<Uint8Array> {
    const line = () => pathLine("read", uri);
    const c = await result(call(() => optional("workspace", line).readFile(uri)), line);
    if (c.tag === "bytes") return c.val;
    throw unexpected(c);
  }

  async readText(uri: Uri): Promise<string> {
    return new TextDecoder().decode(await this.readFile(uri));
  }

  async stat(uri: Uri): Promise<FileStat> {
    const line = () => pathLine("read", uri);
    const c = await result(call(() => optional("workspace", line).stat(uri)), line);
    if (c.tag === "stat") return { isDir: c.val.isDir, size: Number(c.val.size), modified: Number(c.val.modified) };
    throw unexpected(c);
  }

  /** Files matching `glob` under the workspace folders, at most `max`. */
  async findFiles(glob: string, max = 1000): Promise<Uri[]> {
    const line = () => `read = ["${glob}"]`;
    const c = await result(call(() => optional("workspace", line).findFiles(glob, max)), line);
    if (c.tag === "uris") return c.val;
    throw unexpected(c);
  }

  /** Search the workspace's files; matches are passed to `found` as they arrive. */
  async search(pattern: string, found: (m: SearchMatch) => void, options: SearchOptions = {}): Promise<void> {
    const line = () => `read = ["${options.include?.[0] ?? "<glob>"}"]`;
    const query = {
      pattern,
      regex: options.regex ?? false,
      caseSensitive: options.caseSensitive ?? false,
      wholeWord: options.wholeWord ?? false,
      include: options.include ?? [],
      max: options.max ?? 1000,
    };
    const onEvent = (e: StreamEvent) => {
      if (e.tag === "search-match") found(e.val);
    };
    await result(call(() => optional("workspace", line).search(query), onEvent), line);
  }

  async writeFile(uri: Uri, contents: string | Uint8Array): Promise<void> {
    const line = () => pathLine("write", uri);
    await result(call(() => optional("workspaceWrite", line).writeFile(uri, utf8(contents))), line);
  }

  async deleteFile(uri: Uri): Promise<void> {
    const line = () => pathLine("write", uri);
    await result(call(() => optional("workspaceWrite", line).delete(uri)), line);
  }

  // ---- network (`net`, `auth`) -------------------------------------------------------------------------------------

  /** HTTPS to a host listed in `net`. The same as the global `fetch`. */
  fetch(input: string | URL | { url: string }, init?: FetchInit): Promise<FetchResponse> {
    return fetch(input, init);
  }

  /** Like `fetch`, signed for an `auth` provider; the extension never sees the token. */
  fetchAs(provider: string, input: string | URL | { url: string }, init: FetchInit = {}): Promise<FetchResponse> {
    return fetch(input, { ...init, auth: provider });
  }

  /**
   * A `fetch` whose requests are signed for `provider`, for npm clients that take one:
   * `new Octokit({ request: { fetch: ctx.authFetch("github") } })`.
   */
  authFetch(provider: string): typeof globalThis.fetch {
    const f = (input: unknown, init?: unknown) => fetch(input as string, { ...(init as FetchInit | undefined), auth: provider });
    return f as unknown as typeof globalThis.fetch;
  }

  /** Sign in to `provider` if needed (the first use asks the user). */
  async session(provider: string, scopes: string[] = []): Promise<Account> {
    const line = () => `auth = ["${provider}"]`;
    const c = await result(call(() => optional("auth", line).session(provider, scopes)), line);
    if (c.tag === "account") return c.val;
    throw unexpected(c);
  }

  // ---- processes (`run`) -------------------------------------------------------------------------------------------

  /** Start a program listed in `run`; read its output as it comes with `for await (const chunk of process)`. */
  spawn(command: Command): Process {
    return new Process(command);
  }

  /** Run a program to the end and collect its output. */
  async run(command: Command): Promise<Output> {
    const p = this.spawn(command);
    const stdout: Uint8Array[] = [];
    const stderr: Uint8Array[] = [];
    for await (const chunk of p) {
      if ("stdout" in chunk) stdout.push(chunk.stdout);
      else stderr.push(chunk.stderr);
    }
    return { code: await p.wait(), stdout: join(stdout), stderr: join(stderr) };
  }

  // ---- secrets, clipboard, terminal --------------------------------------------------------------------------------

  async secret(key: string): Promise<string | undefined> {
    const line = () => "secrets = true";
    const c = await result(call(() => optional("secrets", line).get(key)), line);
    if (c.tag === "text") return c.val;
    throw unexpected(c);
  }

  async setSecret(key: string, value: string): Promise<void> {
    const line = () => "secrets = true";
    await result(call(() => optional("secrets", line).set(key, value)), line);
  }

  async deleteSecret(key: string): Promise<void> {
    const line = () => "secrets = true";
    await result(call(() => optional("secrets", line).delete(key)), line);
  }

  async readClipboard(): Promise<string | undefined> {
    const line = () => `clipboard = "read"`;
    const c = await result(call(() => optional("clipboardRead", line).readText()), line);
    if (c.tag === "text") return c.val;
    throw unexpected(c);
  }

  async writeClipboard(text: string): Promise<void> {
    const line = () => `clipboard = "write"`;
    await result(call(() => optional("clipboardWrite", line).writeText(text)), line);
  }

  /** Type `text` into the active terminal. */
  async sendToTerminal(text: string): Promise<void> {
    const line = () => "terminal = true";
    await result(call(() => optional("terminal", line).sendText(text)), line);
  }
}

/** A `{ payload }` thrown by an import that returns `result<_, error>`, as a CodeyardError. */
export function thrown(e: unknown): unknown {
  const payload = (e as { payload?: unknown } | undefined)?.payload;
  if (payload && typeof payload === "object" && "tag" in payload) return fromWit(payload as Parameters<typeof fromWit>[0]);
  return e;
}

function join(chunks: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

/** The extension's persisted key-value store (50 MiB in all). */
export class Storage {
  get(key: string): Uint8Array | undefined {
    return host().storage.get(key);
  }

  set(key: string, value: string | Uint8Array): void {
    try {
      host().storage.set(key, utf8(value));
    } catch (e) {
      throw thrown(e);
    }
  }

  getText(key: string): string | undefined {
    const b = this.get(key);
    return b === undefined ? undefined : new TextDecoder().decode(b);
  }

  getJson<T = unknown>(key: string): T | undefined {
    const t = this.getText(key);
    return t === undefined ? undefined : (JSON.parse(t) as T);
  }

  setJson(key: string, value: unknown): void {
    this.set(key, JSON.stringify(value));
  }

  delete(key: string): void {
    host().storage.delete(key);
  }

  keys(): string[] {
    return host().storage.keys();
  }
}

/** The extension's in-memory state (`ctx.state`), JSON values by key. */
export class State {
  get<T = unknown>(key: string): T | undefined {
    return handlers.state.get(key) as T | undefined;
  }

  set(key: string, value: unknown): void {
    // Kept as JSON, so what comes back after a reload is what was set.
    handlers.state.set(key, value === undefined ? undefined : JSON.parse(JSON.stringify(value)));
  }

  delete(key: string): void {
    handlers.state.delete(key);
  }

  keys(): string[] {
    return [...handlers.state.keys()];
  }
}

/** Progress shown until ended. */
export class Progress {
  readonly #id: bigint;
  #ended = false;

  constructor(id: bigint) {
    this.#id = id;
  }

  report(message?: string, percent?: number): void {
    host().window.reportProgress(this.#id, message, percent === undefined ? undefined : Math.max(0, Math.min(100, Math.round(percent))));
  }

  /** The user cancelled it (only when started as cancellable). */
  get cancelled(): boolean {
    return handlers.cancelledProgress.has(this.#id);
  }

  end(): void {
    if (this.#ended) return;
    this.#ended = true;
    host().window.endProgress(this.#id);
    handlers.cancelledProgress.delete(this.#id);
  }
}

/** A repeating timer from `ctx.every`. */
export class Timer {
  readonly #id: bigint;

  constructor(id: bigint) {
    this.#id = id;
  }

  clear(): void {
    host().timers.clear(this.#id);
    handlers.timers.delete(this.#id);
  }
}

/** A running program. Iterate it for its output; `wait()` for its exit code. */
export class Process implements AsyncIterable<Chunk> {
  readonly program: string;
  #chunks: Chunk[] = [];
  #done = false;
  #wake: (() => void) | undefined;
  #exit: Promise<number | undefined>;
  #ticket: bigint | undefined;

  constructor(command: Command) {
    this.program = command.program;
    const line = () => `run = ["${command.program}"]`;
    const spec = {
      program: command.program,
      args: command.args ?? [],
      cwd: command.cwd,
      env: Object.entries(command.env ?? {}),
      stdin: command.stdin === undefined ? undefined : utf8(command.stdin),
    };
    const onEvent = (e: StreamEvent) => {
      if (e.tag === "stdout") this.#push({ stdout: e.val });
      else if (e.tag === "stderr") this.#push({ stderr: e.val });
    };
    let started: Promise<Completion>;
    try {
      const ticket = optional("process", line).spawn(spec);
      this.#ticket = ticket;
      started = wait(ticket, onEvent);
    } catch (e) {
      started = Promise.reject(e);
    }
    this.#exit = started.then(
      (c) => {
        this.#finish();
        if (c.tag === "failed") throw fromWit(c.val, line);
        if (c.tag === "exit") return c.val;
        throw unexpected(c);
      },
      (e) => {
        this.#finish();
        throw e;
      },
    );
    // Reading the output without waiting must not report an unhandled rejection.
    this.#exit.catch(() => {});
  }

  #push(chunk: Chunk): void {
    this.#chunks.push(chunk);
    this.#wakeUp();
  }

  #finish(): void {
    this.#done = true;
    this.#wakeUp();
  }

  #wakeUp(): void {
    const w = this.#wake;
    this.#wake = undefined;
    w?.();
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<Chunk> {
    for (;;) {
      const c = this.#chunks.shift();
      if (c) {
        yield c;
        continue;
      }
      if (this.#done) return;
      await new Promise<void>((r) => {
        this.#wake = r;
      });
    }
  }

  /** The exit code (undefined when killed by a signal); rejects when the program could not start. */
  wait(): Promise<number | undefined> {
    return this.#exit;
  }

  /** Stop it: the host kills the program. */
  kill(): void {
    if (this.#ticket !== undefined) host().tickets.cancel(this.#ticket);
  }
}

export { fromWitDiagnostic, hostOf };
