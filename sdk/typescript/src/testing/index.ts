// `@codeyard/extension/testing`: a mock Codeyard for an extension's unit tests under Node (`npm test`), the same as
// `codeyard-extension-test` for Rust (docs/extensions/testing.md). Fake files, fixture HTTP responses, recorded
// processes, scripted answers to pickers and inputs, and a virtual clock for timers. Calls are checked against the
// extension's `codeyard-extension.toml`, so a test fails where Codeyard would refuse the call, with the same fix.
//
//   const host = new MockHost();
//   await host.activate(extension);
//   host.answerInput("Ada");
//   assert.equal(await host.command("greet"), 1);
//   assert.deepEqual(host.notices(), ["Hello, Ada!"]);
//
// The extension runs through the same SDK code as in Codeyard: the mock implements the host's interfaces, delivers
// each result after the call returns (as the extension host does), and is the global `fetch` from the moment it is
// made until `dispose()`. Every call is checked with Codeyard's own rules (./rules.ts, a copy checked against the Rust
// rules by test/rules.test.ts).

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { CodeyardError, DeniedError, fromWit } from "../errors.ts";
import { activationOutcome, events as watchEvents, installGlobals, start, type ExtensionModule } from "../guest.ts";
import { complete, event, install } from "../runtime.ts";
import {
  fromWitCheckout,
  fromWitComment,
  fromWitContents,
  fromWitDetails,
  fromWitFile,
  fromWitSummary,
  toWitQuery,
  type ChangedFile,
  type Checkout,
  type Comment,
  type FileContents,
  type PullRequest,
  type PullRequestDetails,
  type PullRequestQuery,
  type Remote,
} from "../pulls.ts";
import type {
  CodeAction,
  CodeLens,
  CompletionItem,
  CompletionList,
  Diagnostic,
  Document,
  FormatOptions,
  Hover,
  InlayHint,
  LogLevel,
  Position,
  Range,
  Severity,
  StatusItem,
  TextEdit,
} from "../types.ts";
import { fromWitCodeLens, fromWitDiagnostic, toWitCodeLens, toWitDiagnostic } from "../types.ts";
import type * as W from "../wit.ts";
import { allowsNet, permissionsOf, type Permissions } from "./permissions.ts";
import {
  checkDiagnosticsTarget,
  checkStatusItem,
  checkUrl,
  globsAllow,
  ipAddressLine,
  languageId,
  LIMITS,
  normalize as normalizePath,
  outsideLine,
  pathMatches,
  relative as relativeTo,
  resultTooLarge,
  shownDiagnosticMessage,
} from "./rules.ts";
import { parseToml, type Table } from "./toml.ts";
import { checkEvent, checkView } from "./view-check.ts";

/** The workspace folder of every mock. */
export const FOLDER = "file:///work";

/** A recorded or scripted run of a program. */
export interface ProcessFixture {
  stdout?: string | Uint8Array;
  stderr?: string | Uint8Array;
  /** Undefined: killed by a signal. Defaults to 0. */
  code?: number | undefined;
}

/** A call that returned a ticket, by its WIT name (`net/fetch`), with its arguments. */
export interface Call {
  name: string;
  args: unknown[];
}

export interface FetchedRequest {
  method: string;
  url: string;
  headers: [string, string][];
  body: string;
  /** The `auth` provider that signed it, if any. */
  auth?: string;
}

interface HttpFixture {
  method: string;
  url: string;
  status: number;
  headers: [string, string][];
  body: Uint8Array;
}

type Delivery =
  | { ticket: bigint; event: W.StreamEvent }
  | { ticket: bigint; complete: W.Completion }
  | { watch: bigint; documents: W.WitDocumentEvent[] }
  | { watch: bigint; files: W.WitFileEvent[] };

/** Watches one extension may have at once, as in Codeyard (`limits::WATCHES`). */
const WATCHES = 16;

const bytes = (v: string | Uint8Array): Uint8Array => (typeof v === "string" ? new TextEncoder().encode(v) : v);
const text = (v: Uint8Array): string => new TextDecoder().decode(v);
const tick = () => new Promise<void>((r) => setImmediate(r));

/** The workspace-relative path of a uri, as Codeyard makes it (`..` and `%2e` resolved); undefined outside the folder. */
function relative(uri: string): string | undefined {
  return relativeTo([FOLDER], uri);
}

/** The host of a URL that passed `checkUrl` (others never get as far as asking). */
function hostOf(url: string): string {
  const r = checkUrl(url);
  return "host" in r ? r.host : "";
}

/** Started by the user, so it may open links (`Request::is_user_action` in Codeyard). */
function isUserAction(r: W.Request): boolean {
  if (r.tag === "pull-requests") return r.val.tag === "add-comment" || r.val.tag === "checkout";
  return ["command", "ui-event", "run-code-action", "run-task", "open-uri"].includes(r.tag);
}

/** The hosts `auth.fetch` signs requests to for the built-in GitHub account. */
const GITHUB_HOSTS = ["api.github.com", "uploads.github.com"];

/** A mock Codeyard for one extension. Set it up and inspect it in any order. */
export class MockHost {
  #permissions: Permissions | undefined;
  /** The local ids of the manifest's `[[status-bar]]` items; undefined without a manifest. */
  #statusIds: string[] | undefined;
  /** The manifest's own `[[languages]]`, which name files no built-in language claims. */
  #ownLanguages: { id: string; extensions: string[]; filenames: string[] }[] = [];
  /** The `languages` of each of the manifest's `[[diagnostics]]`; undefined without a manifest. */
  #diagnosticLanguages: string[][] | undefined;
  /** The request being handled is a user action: it may open links. */
  #userAction = false;
  #pumping: Promise<void> | undefined;
  /** The manifest's `[[oauth]]` providers: the name and the hosts `auth.fetch` may sign requests to. */
  #oauth = new Map<string, { name: string; hosts: string[] }>();
  /** Accounts by provider; one not in the map is signed in as `test-user`. */
  #accounts = new Map<string, { signedIn: string } | "signed-out" | "declines">();
  /** Self-managed instances the user added (#184), as `<provider>@<host>`. */
  #instances = new Set<string>();
  /** Hosts the user allowed for `net-from-setting`. */
  #allowedHosts = new Set<string>();
  #untrusted = false;
  #nextTicket = 0n;
  #nextId = 0n;
  #nextRequest = 0n;
  #queue: Delivery[] = [];
  #responses = new Map<bigint, { tag: "ok"; val: W.Response } | { tag: "err"; val: W.WitError }>();
  #guest: W.Guest | undefined;
  #restore: (() => void) | undefined;
  #clock = 0;

  #files = new Map<string, Uint8Array>();
  #http: HttpFixture[] = [];
  #processes: { program: string; args: string[]; fixture: ProcessFixture }[] = [];
  #active: Document | undefined;
  #selections: Range[] = [];
  #settings = new Map<string, string | undefined>();
  #storage = new Map<string, Uint8Array>();
  #commandResults = new Map<string, string>();
  #secrets = new Map<string, string>();
  #clipboard: string | undefined;
  #inputs: (string | undefined)[] = [];
  #picks: number[][] = [];
  #noticeAnswers: number[] = [];
  #timers = new Map<bigint, { period: number; next: number }>();
  #progress = new Map<bigint, string>();
  #statusItems = new Map<string, StatusItem>();
  #diagnostics = new Map<string, Diagnostic[]>();
  /** Documents opened with `openDocument`, and the watches the extension started. */
  #documents = new Map<string, Document>();
  #documentWatches = new Map<bigint, { text: boolean }>();
  #fileWatches = new Map<bigint, string>();

  #calls: Call[] = [];
  #logs: [LogLevel, string][] = [];
  #notices: [Severity, string][] = [];
  #denied: DeniedError[] = [];
  #fetched: FetchedRequest[] = [];
  #ran: { program: string; args: string[]; cwd?: string; env: [string, string][] }[] = [];
  #applied: [string, number, TextEdit[]][] = [];
  #invalidated: string[] = [];
  #scmRefreshed: string[] = [];
  #terminal: string[] = [];

  /**
   * A mock with the permissions and setting defaults of `codeyard-extension.toml` in the current folder (where
   * `npm test` runs), or of `manifest` when given. Without a manifest, every call is allowed.
   */
  constructor(options: { manifest?: string } = {}) {
    const path = join(process.cwd(), "codeyard-extension.toml");
    const manifest = options.manifest ?? (existsSync(path) ? readFileSync(path, "utf8") : undefined);
    if (manifest !== undefined) this.manifest(manifest);
    // The global `fetch` is the mock's from here on, so a test that fetches before `activate` never reaches the network.
    install(this.#bindings());
    this.#restore = installGlobals();
  }

  /** A mock that allows every call. */
  static allowAll(): MockHost {
    return new MockHost({ manifest: "" }).#allowAll();
  }

  #allowAll(): MockHost {
    this.#permissions = undefined;
    this.#statusIds = undefined;
    this.#diagnosticLanguages = undefined;
    this.#ownLanguages = [];
    return this;
  }

  /** Check calls against this manifest's permissions, and use its setting defaults. */
  manifest(toml: string): this {
    const m: Table = parseToml(toml);
    this.#permissions = permissionsOf(m);
    const bar = (Array.isArray(m["status-bar"]) ? m["status-bar"] : []) as Table[];
    this.#statusIds = bar.map((b) => String(b.id));
    const list = (v: unknown) => (Array.isArray(v) ? v.map(String) : []);
    this.#ownLanguages = ((Array.isArray(m.languages) ? m.languages : []) as Table[]).map((l) => ({ id: String(l.id), extensions: list(l.extensions), filenames: list(l.filenames) }));
    const diagnostics = (Array.isArray(m.diagnostics) ? m.diagnostics : []) as Table[];
    this.#diagnosticLanguages = diagnostics.map((d) => (Array.isArray(d.languages) ? (d.languages as unknown[]).map(String) : []));
    this.#oauth.clear();
    for (const o of (Array.isArray(m.oauth) ? m.oauth : []) as Table[]) {
      const hosts = Array.isArray(o.hosts) ? (o.hosts as unknown[]).map(String) : [];
      this.#oauth.set(String(o.id), { name: String(o.name ?? o.id), hosts });
    }
    for (const [key, s] of Object.entries((m.settings ?? {}) as Table)) {
      const def = (s as Table).default;
      if (!this.#settings.has(key)) this.#settings.set(key, def === undefined ? undefined : JSON.stringify(def));
    }
    return this;
  }

  /** Refuse sending, writing and running, as in an untrusted folder. */
  untrusted(): this {
    this.#untrusted = true;
    return this;
  }

  // ---- fixtures ----------------------------------------------------------------------------------------------------

  /** A workspace file. A path without a scheme is relative to the workspace folder. */
  file(path: string, contents: string | Uint8Array): this {
    this.#files.set(this.uri(path), bytes(contents));
    return this;
  }

  /** The uri the mock uses for a workspace-relative path. */
  uri(path: string): string {
    return path.includes("://") ? path : `${FOLDER}/${path.replace(/^\/+/, "")}`;
  }

  /**
   * The response to `method url` (any method when `method` is `*`). A body that is not a string or bytes is sent as
   * JSON.
   */
  http(method: string, url: string, status: number, body: unknown = "", headers: Record<string, string> = {}): this {
    const h = Object.entries(headers).map(([k, v]) => [k.toLowerCase(), v] as [string, string]);
    let b: Uint8Array;
    if (typeof body === "string" || body instanceof Uint8Array) b = bytes(body);
    else {
      b = bytes(JSON.stringify(body));
      if (!h.some(([k]) => k === "content-type")) h.push(["content-type", "application/json; charset=utf-8"]);
    }
    this.#http.push({ method, url, status, headers: h, body: b });
    return this;
  }

  /** What running `program` with these exact `args` prints and returns. */
  process(program: string, args: string[], fixture: ProcessFixture): this {
    this.#processes.push({ program, args, fixture });
    return this;
  }

  /** The active editor's document. */
  activeDocument(path: string, language: string, text: string): this {
    this.#active = { uri: this.uri(path), language, version: 1, text };
    return this;
  }

  selections(ranges: Range[]): this {
    this.#selections = ranges;
    return this;
  }

  /** A setting's value, or unset with `undefined`. */
  setting(key: string, value: unknown): this {
    this.#settings.set(key, value === undefined ? undefined : JSON.stringify(value));
    return this;
  }

  /** The result of `ctx.execute(command, …)`. */
  commandResult(command: string, value: unknown): this {
    this.#commandResults.set(command, JSON.stringify(value ?? null));
    return this;
  }

  secret(key: string, value: string): this {
    this.#secrets.set(key, value);
    return this;
  }

  // ---- accounts (`auth`): every provider starts signed in as `test-user` -------------------------------------------

  /** Signed in to `provider` as `label`: `ctx.session` returns it at once. */
  signIn(provider: string, label: string): this {
    this.#accounts.set(provider, { signedIn: label });
    return this;
  }

  /**
   * Not signed in to `provider`, as after **Sign Out** on the extension page: `ctx.fetchAs` fails with Codeyard's
   * "not signed in" error until `ctx.session` signs in again (the user agrees, as `test-user`).
   */
  signedOut(provider: string): this {
    this.#accounts.set(provider, "signed-out");
    return this;
  }

  /**
   * Not signed in to `provider`, and the user declines every sign-in: `ctx.session` rejects with `cancelled`, as in
   * Codeyard when the user picks **Cancel**, and `ctx.fetchAs` fails as for `signedOut`.
   */
  declineSignIn(provider: string): this {
    this.#accounts.set(provider, "declines");
    return this;
  }

  /**
   * A self-managed instance of the `[[oauth]]` provider `provider` on `host`, as the user adds one on the extension's
   * page (#184): its account is `<provider>@<host>`, signs requests to `host` only, and starts signed in.
   */
  instance(provider: string, host: string): this {
    this.#instances.add(`${provider}@${host.toLowerCase()}`);
    return this;
  }

  clipboardText(text: string): this {
    this.#clipboard = text;
    return this;
  }

  // ---- scripted answers (the next input box, pick or notice gets the next answer; none means dismissed) ------------

  answerInput(text: string): this {
    this.#inputs.push(text);
    return this;
  }

  dismissInput(): this {
    this.#inputs.push(undefined);
    return this;
  }

  /** The indices picked in the next quick pick. */
  answerPick(indices: number[]): this {
    this.#picks.push(indices);
    return this;
  }

  /** The user allowed `host` on the extension page, for an extension that declares `[permissions] net-from-setting`. */
  allowHost(host: string): this {
    this.#allowedHosts.add(host.toLowerCase());
    return this;
  }

  /** The button clicked on the next notice that has buttons. */
  answerNotice(index: number): this {
    this.#noticeAnswers.push(index);
    return this;
  }

  // ---- driving the extension ---------------------------------------------------------------------------------------

  /**
   * Call the extension's `activate`, then deliver what it waits on. Rejects with activate's error, also when an
   * `async activate` rejects after an `await`, as Codeyard then fails the activation.
   */
  async activate(extension: ExtensionModule, reason = "command"): Promise<void> {
    this.#guest = start(extension, this.#bindings());
    this.#restore ??= installGlobals();
    try {
      this.#guest.activate({ reason });
    } catch (e) {
      throw new Error(String(e));
    }
    await this.#pump();
    const outcome = activationOutcome();
    if (!outcome) throw new Error("activate did not finish: it waits on something the mock host never finishes");
    if (outcome.error !== undefined) throw new Error(`activate failed: ${outcome.error}`);
  }

  async deactivate(): Promise<void> {
    this.#need().deactivate();
    await this.#pump();
  }

  /**
   * A hot reload as `cyx dev` does it: `deactivate`, then `activate` with the reason `reload`. Storage is kept, and with
   * it what `ctx.state` held.
   */
  async reload(extension: ExtensionModule): Promise<void> {
    await this.deactivate();
    await this.activate(extension, "reload");
  }

  /** Stop being the global `fetch` (dispose mocks in the reverse order they were made, as `t.after` does). */
  dispose(): void {
    this.#restore?.();
    this.#restore = undefined;
  }

  /** Run the manifest's command `id` (its local id) and return its result. */
  async command(id: string, args?: unknown): Promise<any> {
    const r = await this.request({ tag: "command", val: [id, args === undefined ? "" : JSON.stringify(args)] });
    if (r.tag !== "json") throw CodeyardError.other(`command ${id} answered ${r.tag}`);
    return r.val ? JSON.parse(r.val) : null;
  }

  /**
   * Draw view `id`: its tree, checked as Codeyard checks it before drawing (crates/ext-ui), so a tree Codeyard would
   * refuse fails the test with the reason Codeyard shows.
   */
  async render(id: string): Promise<any> {
    const r = await this.request({ tag: "render", val: id });
    if (r.tag !== "view") throw CodeyardError.other(`view ${id} answered ${r.tag}`);
    const verdict = checkView(r.val);
    if (!verdict.ok) throw CodeyardError.other(`Codeyard would not draw view ${id}: ${verdict.message}`);
    return verdict.view;
  }

  /**
   * Send an event to view `id`, in the form Codeyard sends (docs/extensions/reference/ui.md#events):
   * `{ click: { path: ["add"] } }`, `{ change: { path: ["name"], value: { text: "web" } } }`, …
   */
  async viewEvent(id: string, event: unknown): Promise<void> {
    const json = JSON.stringify(event);
    const why = checkEvent(json);
    if (why !== undefined) throw CodeyardError.other(`Codeyard never sends ${json} (it sends events such as {"click": {"path": ["add"]}}): ${why}`);
    await this.request({ tag: "ui-event", val: [id, json] });
  }

  async format(document: Document, range: Range | undefined, options: FormatOptions): Promise<TextEdit[]> {
    const r = await this.request({ tag: "format", val: [document, range, options] });
    if (r.tag !== "edits") throw CodeyardError.other(`the formatter answered ${r.tag}`);
    return r.val;
  }

  async codeActions(document: Document, range: Range, diagnostics: Diagnostic[]): Promise<CodeAction[]> {
    const r = await this.request({ tag: "code-actions", val: [document, range, diagnostics.map(toWitDiagnostic)] });
    if (r.tag !== "code-actions") throw CodeyardError.other(`the code action provider answered ${r.tag}`);
    return r.val.map((a) => ({
      id: a.id,
      title: a.title,
      edits: Object.fromEntries(a.edits),
      ...(a.command === undefined ? {} : { command: a.command }),
      preferred: a.preferred,
      fixes: a.fixes.map(fromWitDiagnostic),
    }));
  }

  /** Ask the hover provider about `position`, as the mouse resting there does. */
  async hover(document: Document, position: Position): Promise<Hover | undefined> {
    const r = await this.request({ tag: "hover", val: [document, position] });
    if (r.tag !== "hover") throw CodeyardError.other(`the hover provider answered ${r.tag}`);
    if (!r.val) return undefined;
    return r.val.range === undefined ? { contents: r.val.contents } : { contents: r.val.contents, range: r.val.range };
  }

  /** Ask the completion provider at `position`; `trigger` is the trigger character typed, if one asked. */
  async completion(document: Document, position: Position, trigger?: string): Promise<Required<CompletionList>> {
    const r = await this.request({ tag: "completion", val: [document, position, trigger] });
    if (r.tag !== "completions") throw CodeyardError.other(`the completion provider answered ${r.tag}`);
    const items: CompletionItem[] = r.val.items.map((i) => {
      const item: CompletionItem = { label: i.label, kind: i.kind, snippet: i.snippet, additionalEdits: i.additionalEdits };
      for (const k of ["detail", "documentation", "insertText", "range", "filterText", "sortText"] as const) {
        if (i[k] !== undefined) Object.assign(item, { [k]: i[k] });
      }
      return item;
    });
    return { items, incomplete: r.val.incomplete };
  }

  // ---- source control providers ------------------------------------------------------------------------------------

  /** A folder: `""` is the mock workspace folder itself. */
  #folder(root: string): string {
    return root === "" ? FOLDER : this.uri(root);
  }

  /** `request.scm-status`: the state of `root` (`""` for the mock folder, a path in it, or a uri) for provider `id`. */
  async scmStatus(id: string, root = ""): Promise<W.WitScmState> {
    const r = await this.request({ tag: "scm-status", val: [id, this.#folder(root)] });
    if (r.tag !== "scm") throw CodeyardError.other(`the source control provider answered ${r.tag}`);
    return r.val;
  }

  /** `request.scm-original`: the original text of `file` for provider `id`. */
  async scmOriginal(id: string, file: string): Promise<string | undefined> {
    const r = await this.request({ tag: "scm-original", val: [id, this.uri(file)] });
    if (r.tag !== "text") throw CodeyardError.other(`the source control provider answered ${r.tag}`);
    return r.val;
  }

  /** `request.scm-validate`: the message for the input box's `text`, if any. */
  async scmValidate(id: string, text: string, root = ""): Promise<string | undefined> {
    const r = await this.request({ tag: "scm-validate", val: [id, this.#folder(root), text] });
    if (r.tag !== "text") throw CodeyardError.other(`the source control provider answered ${r.tag}`);
    return r.val;
  }

  /** Press a button as Source Control does: its command, with the provider, folder, files and input box text. */
  async scmPress(id: string, action: { command: string }, files: string[] = [], message = "", root = ""): Promise<any> {
    return this.command(action.command, { provider: id, root: this.#folder(root), uris: files.map((f) => this.uri(f)), message });
  }

  /** Providers the extension asked to refresh (`ctx.scmRefresh`), in order. */
  scmRefreshed(): string[] {
    return [...this.#scmRefreshed];
  }

  async runCodeAction(id: string): Promise<void> {
    await this.request({ tag: "run-code-action", val: id });
  }

  /** The code lenses for the lines of `document` in `range`, as the editor asks for the lines in view. */
  async codeLenses(document: Document, range: Range): Promise<CodeLens[]> {
    const r = await this.request({ tag: "code-lens", val: [document, range] });
    if (r.tag !== "code-lenses") throw CodeyardError.other(`the code lens provider answered ${r.tag}`);
    return r.val.map(fromWitCodeLens);
  }

  /** Resolve a lens without a command, as the editor does once it is on screen. */
  async resolveCodeLens(document: Document, lens: CodeLens): Promise<CodeLens> {
    const r = await this.request({ tag: "resolve-code-lens", val: [document, toWitCodeLens(lens)] });
    if (r.tag !== "code-lenses" || r.val.length !== 1) throw CodeyardError.other(`resolving a code lens answered ${r.tag}`);
    return fromWitCodeLens(r.val[0]!);
  }

  /** Click a lens: run its command with its arguments, as the editor does (the extension's own commands here). */
  async clickCodeLens(lens: CodeLens): Promise<any> {
    if (!lens.command) throw CodeyardError.invalid("the lens has no command yet; resolve it first");
    return this.command(lens.command.command, lens.command.args);
  }

  async inlayHints(document: Document, range: Range): Promise<InlayHint[]> {
    const r = await this.request({ tag: "inlay-hints", val: [document, range] });
    if (r.tag !== "inlay-hints") throw CodeyardError.other(`the inlay hint provider answered ${r.tag}`);
    return r.val.map((h) => (h.tooltip === undefined ? { position: h.position, label: h.label } : { position: h.position, label: h.label, tooltip: h.tooltip }));
  }

  async openUri(uri: string): Promise<void> {
    await this.request({ tag: "open-uri", val: uri });
  }

  // ---- pull request providers (`ctx.pullRequests`); `remote.provider` picks the provider -------------------------

  /** Send a pull request query and return the provider's answer as WIT (`{ tag: "list", val: [...] }`, …). */
  async pullRequests(query: PullRequestQuery): Promise<W.PullAnswer> {
    const r = await this.request({ tag: "pull-requests", val: toWitQuery(query) });
    if (r.tag !== "pull-requests") throw CodeyardError.other(`the pull request provider answered ${r.tag}`);
    const want = query.tag === "add-comment" ? "done" : query.tag;
    if (r.val.tag !== want) throw CodeyardError.other(`the pull request provider answered ${r.val.tag} to ${query.tag}`);
    return r.val;
  }

  async listPullRequests(remote: Remote): Promise<PullRequest[]> {
    const a = await this.pullRequests({ tag: "list", remote });
    return a.tag === "list" ? a.val.map(fromWitSummary) : [];
  }

  async pullRequestDetails(remote: Remote, id: string): Promise<PullRequestDetails> {
    const a = await this.pullRequests({ tag: "details", remote, id });
    if (a.tag !== "details") throw CodeyardError.other(`unexpected answer ${a.tag}`);
    return fromWitDetails(a.val);
  }

  async pullRequestFiles(remote: Remote, id: string): Promise<ChangedFile[]> {
    const a = await this.pullRequests({ tag: "files", remote, id });
    return a.tag === "files" ? a.val.map(fromWitFile) : [];
  }

  async pullRequestContents(remote: Remote, id: string, file: ChangedFile): Promise<FileContents> {
    const a = await this.pullRequests({ tag: "contents", remote, id, file });
    if (a.tag !== "contents") throw CodeyardError.other(`unexpected answer ${a.tag}`);
    return fromWitContents(a.val);
  }

  async pullRequestComments(remote: Remote, id: string): Promise<Comment[]> {
    const a = await this.pullRequests({ tag: "comments", remote, id });
    return a.tag === "comments" ? a.val.map(fromWitComment) : [];
  }

  /** The user writes a comment on the request. */
  async addPullRequestComment(remote: Remote, id: string, body: string): Promise<void> {
    await this.pullRequests({ tag: "add-comment", remote, id, body });
  }

  /** The user checks the request out: how Codeyard would fetch it. */
  async checkoutPullRequest(remote: Remote, id: string): Promise<Checkout> {
    const a = await this.pullRequests({ tag: "checkout", remote, id });
    if (a.tag !== "checkout") throw CodeyardError.other(`unexpected answer ${a.tag}`);
    return fromWitCheckout(a.val);
  }

  /** Change a setting and tell the extension. */
  async changeSetting(key: string, value: unknown): Promise<void> {
    this.setting(key, value);
    await this.request({ tag: "settings-changed", val: [key] });
  }

  /** The user cancels progress `id` (from `progress()`). */
  async cancelProgress(id: number): Promise<void> {
    await this.request({ tag: "cancel-progress", val: BigInt(id) });
  }

  /** Move the virtual clock forward, firing each timer once per period that ends in that time. */
  async advance(seconds: number): Promise<void> {
    const end = this.#clock + seconds;
    for (;;) {
      let due: [bigint, number] | undefined;
      for (const [id, t] of this.#timers) {
        if (t.next <= end && (!due || t.next < due[1] || (t.next === due[1] && id < due[0]))) due = [id, t.next];
      }
      if (!due) break;
      const [id, at] = due;
      this.#clock = at;
      const t = this.#timers.get(id);
      if (t) t.next += t.period;
      await this.request({ tag: "timer", val: id });
    }
    this.#clock = end;
  }

  // ---- documents and files, for watches ----------------------------------------------------------------------------

  /** The user opens a document: `opened` reaches the extension's document watches. */
  async openDocument(path: string, language: string, text: string): Promise<void> {
    const doc = { uri: this.uri(path), language, version: 1, text };
    this.#documents.set(doc.uri, doc);
    await this.#documentEvent("opened", doc);
  }

  /** The user edits an open document: its version goes up and `changed` reaches document watches. */
  async editDocument(path: string, text: string): Promise<void> {
    const doc = this.#documents.get(this.uri(path));
    if (!doc) throw new Error(`${path} is not open; call host.openDocument first`);
    doc.version += 1;
    doc.text = text;
    await this.#documentEvent("changed", doc);
  }

  /** The user saves an open document: `saved` reaches document watches. */
  async saveDocument(path: string): Promise<void> {
    const doc = this.#documents.get(this.uri(path));
    if (!doc) throw new Error(`${path} is not open; call host.openDocument first`);
    this.#files.set(doc.uri, bytes(doc.text));
    await this.#documentEvent("saved", doc);
  }

  /** A workspace file appears (from Codeyard or anything else): `created` reaches the file watches whose glob matches. */
  async createFile(path: string, contents: string | Uint8Array): Promise<void> {
    this.#files.set(this.uri(path), bytes(contents));
    await this.#fileEvent("created", this.uri(path));
  }

  async changeFile(path: string, contents: string | Uint8Array): Promise<void> {
    this.#files.set(this.uri(path), bytes(contents));
    await this.#fileEvent("changed", this.uri(path));
  }

  async deleteFile(path: string): Promise<void> {
    this.#files.delete(this.uri(path));
    await this.#fileEvent("deleted", this.uri(path));
  }

  /** Events go only where Codeyard sends them: paths the manifest's `read` allows (in untrusted folders too). */
  #readable(uri: string): boolean {
    const rel = relative(uri);
    return rel !== undefined && (!this.#permissions || globsAllow(this.#permissions.read, rel));
  }

  #documentDelivery(watch: bigint, text: boolean, change: W.DocumentChange, doc: Document): Delivery {
    return { watch, documents: [{ change, uri: doc.uri, language: doc.language, version: doc.version, text: text ? doc.text : undefined }] };
  }

  async #documentEvent(change: W.DocumentChange, doc: Document): Promise<void> {
    if (!this.#readable(doc.uri)) return;
    for (const [watch, { text }] of this.#documentWatches) this.#queue.push(this.#documentDelivery(watch, text, change, doc));
    await this.#pump();
  }

  async #fileEvent(change: W.FileChange, uri: string): Promise<void> {
    if (!this.#readable(uri)) return;
    for (const [watch, glob] of this.#fileWatches) {
      // As in Codeyard: the glob against the whole path in the folder.
      if (pathMatches(glob, relative(uri) ?? "")) this.#queue.push({ watch, files: [{ change, uri }] });
    }
    await this.#pump();
  }

  /** A watch the manifest does not allow is refused as the exthost refuses it; past the limit it is `invalid`. */
  #watch(line: string): bigint {
    if (this.#permissions && this.#permissions.read.length === 0) {
      const denied = new DeniedError(line, "not-declared");
      this.#denied.push(denied);
      throw { payload: { tag: "denied", val: { permission: line, why: "not-declared" } } };
    }
    if (this.#documentWatches.size + this.#fileWatches.size >= WATCHES) {
      throw { payload: { tag: "invalid", val: `an extension may have ${WATCHES} watches at once; stop one first` } };
    }
    return ++this.#nextId;
  }

  /** Send any request and wait for its answer. */
  async request(request: W.Request): Promise<W.Response> {
    const id = ++this.#nextRequest;
    this.#userAction = isUserAction(request);
    try {
      this.#need().handle(id, request);
      await this.#pump();
    } finally {
      this.#userAction = false;
    }
    const r = this.#responses.get(id);
    this.#responses.delete(id);
    if (!r) throw CodeyardError.other("the extension did not answer: it waits on something the mock host never finishes");
    if (r.tag === "err") throw fromWit(r.val);
    return r.val;
  }

  #need(): W.Guest {
    if (!this.#guest) throw new Error("call `await host.activate(extension)` first");
    return this.#guest;
  }

  /**
   * Deliver every completion and event the mock has queued, until the extension stops making calls. It also starts on
   * its own when a call is queued outside a request, such as a `fetch` from the test itself.
   */
  #pump(): Promise<void> {
    this.#pumping ??= this.#drain();
    return this.#pumping;
  }

  async #drain(): Promise<void> {
    try {
      for (;;) {
        await tick();
        const d = this.#queue.shift();
        if (!d) {
          await tick();
          if (this.#queue.length === 0) return;
          continue;
        }
        // Before `activate` there is no guest yet, but a call the test itself made (`fetch`) still waits for its answer.
        const to = this.#guest ?? { event, complete };
        if ("event" in d) to.event(d.ticket, d.event);
        else if ("complete" in d) to.complete(d.ticket, d.complete);
        else if ("documents" in d) watchEvents.documents(d.watch, d.documents);
        else watchEvents.files(d.watch, d.files);
      }
    } finally {
      this.#pumping = undefined;
    }
  }

  // ---- what happened -----------------------------------------------------------------------------------------------

  /** The text of every notice shown, in order. */
  notices(): string[] {
    return this.#notices.map(([, m]) => m);
  }

  noticesWithSeverity(): [Severity, string][] {
    return [...this.#notices];
  }

  /** Every call that was refused, with the manifest line that would allow it (`.permission`, `.fix`). */
  denied(): DeniedError[] {
    return [...this.#denied];
  }

  /** Every request the extension sent, fixture or not. */
  fetched(): FetchedRequest[] {
    return [...this.#fetched];
  }

  /** Every program the extension started. */
  ran(): { program: string; args: string[]; cwd?: string; env: [string, string][] }[] {
    return [...this.#ran];
  }

  /** Every call that returned a ticket, in order. */
  calls(): Call[] {
    return [...this.#calls];
  }

  logs(): [LogLevel, string][] {
    return [...this.#logs];
  }

  /** A workspace file as it is now (after the extension's writes). */
  read(path: string): Uint8Array | undefined {
    return this.#files.get(this.uri(path));
  }

  readText(path: string): string | undefined {
    const b = this.read(path);
    return b === undefined ? undefined : text(b);
  }

  /** The extension's storage, values as text. */
  storage(): Record<string, string> {
    return Object.fromEntries([...this.#storage].map(([k, v]) => [k, text(v)]));
  }

  /** The extension's keychain entries (`secrets`), as it left them. */
  secrets(): Record<string, string> {
    return Object.fromEntries(this.#secrets);
  }

  /**
   * As Codeyard: refused with a warning in the extension's log (`logs()`) unless the manifest declares `[[diagnostics]]`
   * for the file's language and the file is in the workspace or open; messages that could disguise what they say are
   * dropped, long ones cut, and one extension has diagnostics on at most `DIAGNOSTIC_FILES` files.
   */
  #setDiagnostics(uri: string, diagnostics: W.Diagnostic[]): void {
    const doc = this.#documents.get(uri) ?? (this.#active?.uri === uri ? this.#active : undefined);
    // By the file's name, wherever it is, as Codeyard does (it gives an open document the language of its file name too,
    // whatever the test said it was).
    const language = languageId(this.#ownLanguages, normalizePath(uri) ?? "");
    const others = [...this.#diagnostics.keys()].filter((u) => u !== uri).length;
    const why = checkDiagnosticsTarget(this.#diagnosticLanguages, language, relative(uri) !== undefined || doc !== undefined, others);
    if (why !== undefined) {
      this.#logs.push(["warn", `set-diagnostics on ${uri} refused: ${why}`]);
      return;
    }
    const kept: Diagnostic[] = [];
    for (const d of diagnostics.map(fromWitDiagnostic)) {
      const m = shownDiagnosticMessage(d.message);
      if (m) kept.push({ ...d, message: m.shown });
    }
    if (kept.length < diagnostics.length) {
      this.#logs.push(["warn", `set-diagnostics on ${uri}: ${diagnostics.length - kept.length} with an empty message or hidden characters dropped`]);
    }
    kept.length = Math.min(kept.length, LIMITS.MARKS_PER_FILE);
    if (kept.length === 0) this.#diagnostics.delete(uri);
    else this.#diagnostics.set(uri, kept);
  }

  /** The status bar items the extension set, by id. */
  statusItems(): Record<string, StatusItem> {
    return Object.fromEntries(this.#statusItems);
  }

  diagnostics(path: string): Diagnostic[] {
    return this.#diagnostics.get(this.uri(path)) ?? [];
  }

  /** Views the extension asked to draw again. */
  invalidated(): string[] {
    return [...this.#invalidated];
  }

  /** Edits applied to documents: uri, the version they were made against, the edits. */
  appliedEdits(): [string, number, TextEdit[]][] {
    return [...this.#applied];
  }

  /** Timers that are running: id and period in seconds. */
  timers(): [number, number][] {
    return [...this.#timers].map(([id, t]) => [Number(id), t.period]);
  }

  /** Progress that has not ended: id and title. */
  progress(): [number, string][] {
    return [...this.#progress].map(([id, t]) => [Number(id), t]);
  }

  clipboard(): string | undefined {
    return this.#clipboard;
  }

  /** Text typed into the terminal. */
  terminal(): string[] {
    return [...this.#terminal];
  }

  // ---- the host's side of every call -------------------------------------------------------------------------------

  /** Whether the broker refuses this call: the manifest line that would allow it. */
  #refusal(name: string, args: unknown[]): DeniedError | undefined {
    const p = this.#permissions;
    const check = (ok: (p: Permissions) => boolean, line: string, sends: boolean) => {
      if (p && !ok(p)) return new DeniedError(line, "not-declared");
      return sends && this.#untrusted ? new DeniedError(line, "untrusted-workspace") : undefined;
    };
    // A path outside the workspace folder is refused whatever the globs say, as Codeyard's broker refuses it.
    const path = (perm: "read" | "write", uri: string, sends: boolean) => {
      const rel = relative(uri);
      if (rel === undefined) return p ? new DeniedError(outsideLine(perm, uri), "not-declared") : undefined;
      return check((p) => globsAllow(p[perm], rel), `${perm} = ["${rel}"]`, sends);
    };
    const uri = args[0] as string;
    const active = this.#active?.uri;
    switch (name) {
      case "workspace/read-file":
      case "workspace/stat":
        return path("read", uri, false);
      case "documents/read":
        return uri === active ? undefined : path("read", uri, false);
      case "workspace/find-files":
        return check((p) => p.read.length > 0, `read = ["${uri}"]`, false);
      case "workspace-write/write-file":
      case "workspace-write/delete":
      case "workspace-write/create-directory":
        return path("write", uri, true);
      case "workspace-write/rename":
        return path("write", uri, true) ?? path("write", args[1] as string, true);
      // A command may edit the active document without `write` (docs/plugin-architecture.md §5.2).
      case "documents/apply-edits":
        return uri === active ? undefined : path("write", uri, true);
      case "net/fetch": {
        const host = hostOf((args[0] as W.HttpRequest).url);
        // A host the user allowed for `net-from-setting` counts as declared; another is theirs to allow, not the author's.
        if (p?.netFromSetting !== undefined && !allowsNet(p, host)) {
          return check(() => this.#allowedHosts.has(host), `net-from-setting host = "${host}"`, true);
        }
        return check((p) => allowsNet(p, host), `net = ["${host}"]`, true);
      }
      // An instance's account (`gitlab@git.example.com`) is granted by its provider's entry, once the user added it.
      case "auth/session":
        return this.#accountRefusal(uri);
      case "auth/fetch":
        return this.#accountRefusal(uri) ?? this.#unsignedHost(uri, (args[1] as W.HttpRequest).url);
      case "process/spawn": {
        const program = (args[0] as W.ProcessCommand).program;
        return check((p) => p.run.includes(program), `run = ["${program}"]`, true);
      }
      case "secrets/get":
      case "secrets/set":
      case "secrets/delete":
        return check((p) => p.secrets, "secrets = true", false);
      case "clipboard-read/read-text":
        return check((p) => p.clipboard === "read" || p.clipboard === "read-write", `clipboard = "read"`, false);
      case "clipboard-write/write-text":
        return check((p) => p.clipboard === "write" || p.clipboard === "read-write", `clipboard = "write"`, true);
      case "terminal/send-text":
        return check((p) => p.terminal, "terminal = true", true);
      case "commands/execute":
        return uri.startsWith("core:") ? check((p) => p.commands.includes(uri), `commands = ["${uri}"]`, false) : undefined;
      default:
        return undefined;
    }
  }

  /**
   * `auth.fetch` signs requests only to the hosts of the provider (`[[oauth]] hosts`, GitHub's API hosts), as Codeyard
   * does: a request to any other host is refused and not sent.
   */
  #accountRefusal(account: string): DeniedError | undefined {
    const base = account.split("@")[0]!;
    const line = `auth = ["${base}"]`;
    const declared = (!this.#permissions || this.#permissions.auth.includes(base)) && (!account.includes("@") || this.#instances.has(account));
    if (!declared) return new DeniedError(line, "not-declared");
    return this.#untrusted ? new DeniedError(line, "untrusted-workspace") : undefined;
  }

  #unsignedHost(provider: string, url: string): DeniedError | undefined {
    if (!this.#permissions) return undefined;
    const instance = provider.includes("@") ? { name: provider, hosts: [provider.slice(provider.indexOf("@") + 1)] } : undefined;
    const o = instance ?? this.#oauth.get(provider) ?? (provider === "github" ? { name: "GitHub", hosts: GITHUB_HOSTS } : undefined);
    const host = hostOf(url);
    if (!o || allowsNet({ net: o.hosts } as Permissions, host)) return undefined;
    return new DeniedError(`[[oauth]] ${provider} hosts = ["${host}"]`, "not-declared");
  }

  /**
   * Start a call: record it, refuse it or answer it, and queue the answer for after the call returns. `early` is what
   * Codeyard answers before it looks at permissions: `invalid` for a URL it never fetches, a denial no entry can lift for
   * one that names an IP address.
   */
  #start(name: string, args: unknown[], answer: () => [W.StreamEvent[], W.Completion], early?: string | DeniedError): bigint {
    const ticket = ++this.#nextTicket;
    this.#calls.push({ name, args });
    const invalid = typeof early === "string" ? early : undefined;
    const refused = early instanceof DeniedError ? early : invalid === undefined ? this.#refusal(name, args) : undefined;
    if (invalid !== undefined) {
      this.#queue.push({ ticket, complete: { tag: "failed", val: { tag: "invalid", val: invalid } } });
    } else if (refused) {
      this.#denied.push(refused);
      this.#queue.push({ ticket, complete: { tag: "failed", val: { tag: "denied", val: { permission: refused.permission, why: refused.why } } } });
    } else {
      const [events, completion] = answer();
      for (const event of events) this.#queue.push({ ticket, event });
      this.#queue.push({ ticket, complete: completion });
    }
    void this.#pump();
    return ticket;
  }

  #fetch(name: string, provider: string | undefined, r: W.HttpRequest): bigint {
    const url = checkUrl(r.url);
    const early = !("message" in url) ? undefined : url.problem === "ip-literal" ? new DeniedError(ipAddressLine(url.host), "not-declared") : url.message;
    return this.#start(name, provider === undefined ? [r] : [provider, r], () => {
      if (provider !== undefined && r.headers.some(([n]) => n.trim().toLowerCase() === "authorization")) {
        return [[], { tag: "failed", val: { tag: "invalid", val: "the authorization header is added by Codeyard; remove it from the request" } }];
      }
      const account = provider === undefined ? undefined : this.#accounts.get(provider);
      if (account === "signed-out" || account === "declines") {
        // Codeyard's words when the keychain has no account for the provider.
        const name = this.#oauth.get(provider!)?.name ?? (provider === "github" ? "GitHub" : provider);
        return [[], { tag: "failed", val: { tag: "invalid", val: `not signed in to ${name}; call auth.session with “${provider}” first` } }];
      }
      this.#fetched.push({ method: r.method, url: r.url, headers: r.headers, body: text(r.body), ...(provider ? { auth: provider } : {}) });
      const f = this.#http.find((f) => (f.method === "*" || f.method.toUpperCase() === r.method.toUpperCase()) && f.url === r.url);
      if (!f) {
        const msg = `the mock host has no response for ${r.method} ${r.url}; add one with host.http(…)`;
        return [[], { tag: "failed", val: { tag: "other", val: msg } }];
      }
      return [
        [
          { tag: "http-head", val: { status: f.status, headers: f.headers } },
          { tag: "data", val: f.body },
        ],
        { tag: "done" },
      ];
    }, early);
  }

  #bindings(): W.Bindings {
    const done: W.Completion = { tag: "done" };
    const notFound = (what: string): W.Completion => ({ tag: "failed", val: { tag: "not-found", val: what } });
    const id = () => ++this.#nextId;
    return {
      log: { log: (level, message) => void this.#logs.push([level, message]) },
      env: {
        info: () => ({
          codeyardVersion: "0.5.0",
          apiVersion: "1.1.0",
          language: "en",
          theme: "dark",
          platform: process.platform === "darwin" ? "macos" : process.platform === "win32" ? "windows" : "linux",
          remote: false,
          workspaceTrusted: !this.#untrusted,
        }),
      },
      tickets: { cancel: () => {} },
      requests: {
        // An answer over the call limit never reaches Codeyard: the request fails, as from the extension host.
        respond: (rid, result) => {
          const r = result.tag === "ok" ? result.val : undefined;
          const text = r && (r.tag === "json" || r.tag === "view" || r.tag === "text") ? (r.val ?? "") : "";
          const tooBig = resultTooLarge(new TextEncoder().encode(text).length);
          this.#responses.set(rid, tooBig ? { tag: "err", val: { tag: "invalid", val: tooBig } } : result);
        },
      },
      window: {
        showNotice: (severity, message, actions) =>
          this.#start("window/show-notice", [severity, message, actions], () => {
            this.#notices.push([severity, message]);
            const chosen = actions.length ? this.#noticeAnswers.shift() : undefined;
            return [[], { tag: "chosen", val: chosen === undefined ? [] : [chosen] }];
          }),
        quickPick: (items, options) => this.#start("window/quick-pick", [items, options], () => [[], { tag: "chosen", val: this.#picks.shift() ?? [] }]),
        inputBox: (options) => this.#start("window/input-box", [options], () => [[], { tag: "text", val: this.#inputs.shift() }]),
        startProgress: (title) => {
          const p = id();
          this.#progress.set(p, title);
          return p;
        },
        reportProgress: () => {},
        endProgress: (p) => void this.#progress.delete(p),
        // Refused as Codeyard refuses it: an id not declared in `[[status-bar]]`, an empty or too long text.
        setStatusItem: (item) => {
          const verdict = checkStatusItem(this.#statusIds, item.id, item.text, item.tooltip);
          if ("error" in verdict) throw { payload: { tag: "invalid", val: verdict.error } };
          const kept = { ...item, text: verdict.shown };
          this.#statusItems.set(item.id, Object.fromEntries(Object.entries(kept).filter(([, v]) => v !== undefined)) as unknown as StatusItem);
        },
        removeStatusItem: (sid) => void this.#statusItems.delete(sid),
        // As in Codeyard: a link opens only while the extension handles a user action.
        openExternal: (url) =>
          this.#start("window/open-external", [url], () => {
            if (!this.#userAction) {
              return [[], { tag: "failed", val: { tag: "denied", val: { permission: "a user action (a command or a link click)", why: "not-declared" } } }];
            }
            if (!url.startsWith("https://")) return [[], { tag: "failed", val: { tag: "invalid", val: "only https: links open" } }];
            return [[], done];
          }),
      },
      commands: {
        execute: (command, argsJson) => this.#start("commands/execute", [command, argsJson], () => [[], { tag: "json", val: this.#commandResults.get(command) ?? "null" }]),
      },
      storage: {
        get: (key) => this.#storage.get(key),
        set: (key, value) => {
          let total = key.length + value.length;
          for (const [k, v] of this.#storage) if (k !== key) total += k.length + v.length;
          if (total > LIMITS.DATA_DIR_BYTES) throw { payload: { tag: "invalid", val: "the store would grow past its 50 MiB limit" } };
          this.#storage.set(key, value);
        },
        delete: (key) => void this.#storage.delete(key),
        keys: () => [...this.#storage.keys()].sort(),
      },
      settings: { get: (key) => this.#settings.get(key) },
      timers: {
        every: (seconds) => {
          const t = id();
          const period = Math.max(seconds, LIMITS.TIMER_MIN_SECONDS);
          this.#timers.set(t, { period, next: this.#clock + period });
          return t;
        },
        clear: (t) => void this.#timers.delete(t),
      },
      ui: { invalidate: (view) => void this.#invalidated.push(view) },
      scm: { refresh: (provider) => void this.#scmRefreshed.push(provider) },
      watch: {
        documents: (text) => {
          const w = this.#watch(`read = ["<glob>"]`);
          this.#documentWatches.set(w, { text });
          // The documents already open arrive first, as in Codeyard.
          for (const doc of this.#documents.values()) {
            if (this.#readable(doc.uri)) this.#queue.push(this.#documentDelivery(w, text, "opened", doc));
          }
          return w;
        },
        files: (glob) => {
          const w = this.#watch(`read = ["${glob}"]`);
          this.#fileWatches.set(w, glob);
          return w;
        },
        stop: (w) => void (this.#documentWatches.delete(w) || this.#fileWatches.delete(w)),
      },
      documents: {
        active: () => this.#start("documents/active", [], () => [[], { tag: "document", val: this.#active }]),
        selections: () => this.#start("documents/selections", [], () => [[], { tag: "ranges", val: this.#selections }]),
        read: (uri) => this.#start("documents/read", [uri], () => [[], { tag: "document", val: this.#active?.uri === uri ? this.#active : undefined }]),
        applyEdits: (uri, version, edits) =>
          this.#start("documents/apply-edits", [uri, version, edits], () => {
            const d = this.#active;
            if (d && d.uri === uri) {
              if (d.version !== version) return [[], { tag: "failed", val: { tag: "invalid", val: `the document is at version ${d.version}, not ${version}` } }];
              d.version += 1;
            }
            this.#applied.push([uri, version, edits]);
            return [[], done];
          }),
        setSelections: (uri, selections) => this.#start("documents/set-selections", [uri, selections], () => [[], done]),
      },
      languages: {
        setDiagnostics: (uri, diagnostics) => this.#setDiagnostics(uri, diagnostics),
        clearDiagnostics: (uri) => (uri === undefined ? this.#diagnostics.clear() : void this.#diagnostics.delete(uri)),
        setDecorations: () => {},
      },
      workspace: {
        folders: () => (this.#permissions && this.#permissions.read.length === 0 ? [] : [FOLDER]),
        readFile: (uri) =>
          this.#start("workspace/read-file", [uri], () => {
            const b = this.#files.get(uri);
            return [[], b ? { tag: "bytes", val: b } : notFound(uri)];
          }),
        readMany: (uris) =>
          this.#start("workspace/read-many", [uris], () => [
            [],
            {
              tag: "files",
              val: uris.map((u) => {
                const b = this.#files.get(u);
                return { uri: u, contents: b ? { tag: "ok", val: b } : { tag: "err", val: { tag: "not-found", val: u } } };
              }),
            },
          ]),
        stat: (uri) =>
          this.#start("workspace/stat", [uri], () => {
            const b = this.#files.get(uri);
            return [[], b ? { tag: "stat", val: { isDir: false, size: BigInt(b.length), modified: 0n } } : notFound(uri)];
          }),
        // As in Codeyard: files the glob matches and `read` allows.
        findFiles: (glob, max) =>
          this.#start("workspace/find-files", [glob, max], () => {
            const readable = (rel: string) => !this.#permissions || globsAllow(this.#permissions.read, rel);
            const found = [...this.#files.keys()].filter((u) => {
              const rel = relative(u);
              return rel !== undefined && pathMatches(glob, rel) && readable(rel);
            });
            return [[], { tag: "uris", val: found.slice(0, max) }];
          }),
        search: (query) =>
          this.#start("workspace/search", [query], () => {
            const events: W.StreamEvent[] = [];
            for (const [uri, b] of this.#files) {
              text(b)
                .split("\n")
                .forEach((line, n) => {
                  const at = query.caseSensitive ? line.indexOf(query.pattern) : line.toLowerCase().indexOf(query.pattern.toLowerCase());
                  if (at >= 0 && events.length < query.max) {
                    const range = { start: { line: n, character: at }, end: { line: n, character: at + query.pattern.length } };
                    events.push({ tag: "search-match", val: { uri, range, line } });
                  }
                });
            }
            return [events, done];
          }),
      },
      workspaceWrite: {
        writeFile: (uri, contents) =>
          this.#start("workspace-write/write-file", [uri, contents], () => {
            this.#files.set(uri, contents);
            return [[], done];
          }),
        delete: (uri) => this.#start("workspace-write/delete", [uri], () => [[], this.#files.delete(uri) ? done : notFound(uri)]),
        rename: (from, to) =>
          this.#start("workspace-write/rename", [from, to], () => {
            const b = this.#files.get(from);
            if (!b) return [[], notFound(from)];
            this.#files.delete(from);
            this.#files.set(to, b);
            return [[], done];
          }),
        createDirectory: (uri) => this.#start("workspace-write/create-directory", [uri], () => [[], done]),
      },
      net: { fetch: (r) => this.#fetch("net/fetch", undefined, r) },
      auth: {
        // The first use asks; the user agrees (as `test-user`) unless the test said they decline, which Codeyard reports
        // as `cancelled`.
        session: (provider, scopes) =>
          this.#start("auth/session", [provider, scopes], () => {
            const a = this.#accounts.get(provider);
            if (a === "declines") return [[], { tag: "failed", val: { tag: "cancelled" } }];
            const label = typeof a === "object" ? a.signedIn : "test-user";
            this.#accounts.set(provider, { signedIn: label });
            return [[], { tag: "account", val: { provider, label } }];
          }),
        fetch: (provider, r) => this.#fetch("auth/fetch", provider, r),
      },
      secrets: {
        get: (key) => this.#start("secrets/get", [key], () => [[], { tag: "text", val: this.#secrets.get(key) }]),
        set: (key, value) =>
          this.#start("secrets/set", [key], () => {
            this.#secrets.set(key, value);
            return [[], done];
          }),
        delete: (key) =>
          this.#start("secrets/delete", [key], () => {
            this.#secrets.delete(key);
            return [[], done];
          }),
      },
      process: {
        spawn: (c) =>
          this.#start("process/spawn", [c], () => {
            this.#ran.push({ program: c.program, args: c.args, ...(c.cwd === undefined ? {} : { cwd: c.cwd }), env: c.env });
            const f = this.#processes.find((p) => p.program === c.program && p.args.length === c.args.length && p.args.every((a, n) => a === c.args[n]));
            if (!f) {
              const msg = `the mock host has no result for \`${[c.program, ...c.args].join(" ")}\`; add one with host.process(…)`;
              return [[], { tag: "failed", val: { tag: "other", val: msg } }];
            }
            const events: W.StreamEvent[] = [];
            if (f.fixture.stdout !== undefined && f.fixture.stdout.length) events.push({ tag: "stdout", val: bytes(f.fixture.stdout) });
            if (f.fixture.stderr !== undefined && f.fixture.stderr.length) events.push({ tag: "stderr", val: bytes(f.fixture.stderr) });
            return [events, { tag: "exit", val: "code" in f.fixture ? f.fixture.code : 0 }];
          }),
      },
      clipboardRead: { readText: () => this.#start("clipboard-read/read-text", [], () => [[], { tag: "text", val: this.#clipboard }]) },
      clipboardWrite: {
        writeText: (t) =>
          this.#start("clipboard-write/write-text", [t], () => {
            this.#clipboard = t;
            return [[], done];
          }),
      },
      terminal: {
        sendText: (t) =>
          this.#start("terminal/send-text", [t], () => {
            this.#terminal.push(t);
            return [[], done];
          }),
      },
    };
  }
}

export { CodeyardError, DeniedError };
