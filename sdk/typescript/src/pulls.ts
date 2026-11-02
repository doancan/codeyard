// Pull (merge) request providers: `[[pull-requests]]` in the manifest, registered with `ctx.pullRequests(id, …)`. The
// author's values (plain numbers, optional fields) and their conversions to and from the WIT's `pull-requests`
// interface, and the answer to each query.

import type { Context } from "./context.ts";
import { CodeyardError } from "./errors.ts";
import { handlers } from "./handlers.ts";
import type * as W from "./wit.ts";

type Awaitable<T> = T | Promise<T>;

/** A repository remote whose host the provider claims. */
export interface Remote {
  /** The provider's local id, from `[[pull-requests]] id`. */
  provider: string;
  /** The remote's name in the repository, such as `origin`. */
  name: string;
  /** The URL as git has it: `git@gitlab.com:group/app.git`. */
  url: string;
  /** The host, lowercase: `gitlab.com`. */
  host: string;
  /** The repository's path on the host, without slashes around it and without `.git`: `group/sub/app`. */
  path: string;
  /** The `auth` provider to sign requests for (`ctx.fetchAs(remote.account, …)`); missing when the provider has none. */
  account?: string;
}

export type PullRequestState = W.PullState;

export interface PullRequest {
  /** The provider's id for the request, passed back in later queries. */
  id: string;
  /** Shown before the title, such as `#42` or `!42`. */
  reference: string;
  title: string;
  author: string;
  state: PullRequestState;
  /** The branch with the changes. */
  sourceBranch: string;
  /** The branch it merges into. */
  targetBranch: string;
  /** Last change, in Unix milliseconds. Defaults to 0 (unknown). */
  updated?: number;
  /** The request's web page (`https:` only). */
  url?: string;
}

export type CheckState = W.PullCheckState;

/** A CI result. */
export interface Check {
  name: string;
  state: CheckState;
  /** `https:` only. */
  url?: string;
}

export interface PullRequestDetails {
  summary: PullRequest;
  /** Markdown. */
  description: string;
  checks?: Check[];
  labels?: string[];
}

export type FileStatus = W.PullFileStatus;

export interface ChangedFile {
  /** The path in the source branch (in the target branch for a deleted file). */
  path: string;
  /** The path in the target branch, for a renamed file. */
  oldPath?: string;
  status: FileStatus;
}

/** One changed file's text on both sides. */
export interface FileContents {
  /** In the target branch: missing for an added or binary file. */
  base?: string;
  /** In the source branch: missing for a deleted or binary file. */
  head?: string;
}

export interface Comment {
  author: string;
  /** Markdown. */
  body: string;
  /** Unix milliseconds. Defaults to 0 (unknown). */
  created?: number;
  /** For a comment on the diff: the file, and the line (1-based) in the source branch. */
  path?: string;
  line?: number;
}

/** How to check a request out: Codeyard runs `git fetch <fetchUrl or the remote> <ref>` and creates or resets `branch`. */
export interface Checkout {
  /** A clone URL to fetch from (a fork); missing for the remote itself. */
  fetchUrl?: string;
  /** What to fetch: `refs/pull/42/head`, `refs/merge-requests/42/head`, or a branch name. */
  ref: string;
  /** The local branch, such as `pr-42`. */
  branch: string;
}

/** What `ctx.pullRequests(id, provider)` registers. */
export interface PullRequestProvider {
  /** Open requests of the remote's repository, newest first. */
  list(ctx: Context, remote: Remote): Awaitable<PullRequest[]>;
  details(ctx: Context, remote: Remote, id: string): Awaitable<PullRequestDetails>;
  files(ctx: Context, remote: Remote, id: string): Awaitable<ChangedFile[]>;
  contents(ctx: Context, remote: Remote, id: string, file: ChangedFile): Awaitable<FileContents>;
  /** Its comments, oldest first. Without it, a request has none. */
  comments?(ctx: Context, remote: Remote, id: string): Awaitable<Comment[]>;
  /** The user wrote a comment (Markdown). Without it, commenting is refused. */
  addComment?(ctx: Context, remote: Remote, id: string, body: string): Awaitable<void>;
  /** The user wants the request checked out. Without it, checking out is refused. */
  checkout?(ctx: Context, remote: Remote, id: string): Awaitable<Checkout>;
}

const REQUIRED = ["list", "details", "files", "contents"] as const;

/** `ctx.pullRequests`: check the provider has its required handlers and register it. */
export function registerPullRequests(id: string, provider: PullRequestProvider): void {
  for (const name of REQUIRED) {
    if (typeof provider?.[name] !== "function") {
      throw CodeyardError.invalid(`the pull request provider '${id}' needs a ${name}(ctx, …) handler`);
    }
  }
  handlers.pullRequests.set(id, provider);
}

// ---- conversions -------------------------------------------------------------------------------------------------

/** `{ key: v }`, or nothing when `v` is undefined: optional fields are left out, not set to undefined. */
const opt = <K extends string, V>(key: K, v: V | undefined): { [P in K]?: V } => (v === undefined ? {} : ({ [key]: v } as { [P in K]?: V }));

/** Unix milliseconds as the WIT's u64; anything else is 0. */
const ms = (n: number | undefined): bigint => (typeof n === "number" && Number.isFinite(n) && n > 0 ? BigInt(Math.trunc(n)) : 0n);

export function fromWitRemote(r: W.PullRemote): Remote {
  return { provider: r.provider, name: r.name, url: r.url, host: r.host, path: r.path, ...opt("account", r.account) };
}

export function toWitRemote(r: Remote): W.PullRemote {
  return { provider: r.provider, name: r.name, url: r.url, host: r.host, path: r.path, account: r.account };
}

export function toWitSummary(p: PullRequest): W.PullSummary {
  return {
    id: p.id,
    reference: p.reference,
    title: p.title,
    author: p.author,
    state: p.state,
    sourceBranch: p.sourceBranch,
    targetBranch: p.targetBranch,
    updated: ms(p.updated),
    url: p.url,
  };
}

export function fromWitSummary(p: W.PullSummary): PullRequest {
  const { updated, url, ...rest } = p;
  return { ...rest, updated: Number(updated), ...opt("url", url) };
}

export function toWitDetails(d: PullRequestDetails): W.PullDetails {
  return {
    summary: toWitSummary(d.summary),
    description: d.description,
    checks: (d.checks ?? []).map((c) => ({ name: c.name, state: c.state, url: c.url })),
    labels: d.labels ?? [],
  };
}

export function fromWitDetails(d: W.PullDetails): PullRequestDetails {
  return {
    summary: fromWitSummary(d.summary),
    description: d.description,
    checks: d.checks.map((c) => ({ name: c.name, state: c.state, ...opt("url", c.url) })),
    labels: d.labels,
  };
}

export function toWitFile(f: ChangedFile): W.PullChangedFile {
  return { path: f.path, oldPath: f.oldPath, status: f.status };
}

export function fromWitFile(f: W.PullChangedFile): ChangedFile {
  return { path: f.path, ...opt("oldPath", f.oldPath), status: f.status };
}

export function toWitContents(c: FileContents): W.PullFileContents {
  return { base: c.base, head: c.head };
}

export function fromWitContents(c: W.PullFileContents): FileContents {
  return { ...opt("base", c.base), ...opt("head", c.head) };
}

export function toWitComment(c: Comment): W.PullComment {
  return { author: c.author, body: c.body, created: ms(c.created), path: c.path, line: c.line };
}

export function fromWitComment(c: W.PullComment): Comment {
  return { author: c.author, body: c.body, created: Number(c.created), ...opt("path", c.path), ...opt("line", c.line) };
}

export function toWitCheckout(c: Checkout): W.PullCheckout {
  return { fetchUrl: c.fetchUrl, ref: c.ref, branch: c.branch };
}

export function fromWitCheckout(c: W.PullCheckout): Checkout {
  return { ...opt("fetchUrl", c.fetchUrl), ref: c.ref, branch: c.branch };
}

/** The query as WIT, from the author's values (for the mock host). */
export function toWitQuery(q: PullRequestQuery): W.PullQuery {
  switch (q.tag) {
    case "list":
      return { tag: "list", val: toWitRemote(q.remote) };
    case "contents":
      return { tag: "contents", val: [toWitRemote(q.remote), q.id, toWitFile(q.file)] };
    case "add-comment":
      return { tag: "add-comment", val: [toWitRemote(q.remote), q.id, q.body] };
    default:
      return { tag: q.tag, val: [toWitRemote(q.remote), q.id] };
  }
}

/** A query with the author's values. */
export type PullRequestQuery =
  | { tag: "list"; remote: Remote }
  | { tag: "details" | "files" | "comments" | "checkout"; remote: Remote; id: string }
  | { tag: "contents"; remote: Remote; id: string; file: ChangedFile }
  | { tag: "add-comment"; remote: Remote; id: string; body: string };

// ---- answering ---------------------------------------------------------------------------------------------------

/** Answer a `request.pull-requests` query with the provider its remote names. Rejects with the error to send back. */
export async function answerPullRequests(ctx: Context, query: W.PullQuery): Promise<W.PullAnswer> {
  const raw = query.tag === "list" ? query.val : query.val[0];
  const name = raw.provider;
  const p = handlers.pullRequests.get(name);
  if (!p) throw CodeyardError.notFound(`no pull request provider '${name}' is registered; call ctx.pullRequests("${name}", …) in activate`);
  const remote = fromWitRemote(raw);
  switch (query.tag) {
    case "list":
      return { tag: "list", val: ((await p.list(ctx, remote)) ?? []).map(toWitSummary) };
    case "details":
      return { tag: "details", val: toWitDetails(await p.details(ctx, remote, query.val[1])) };
    case "files":
      return { tag: "files", val: ((await p.files(ctx, remote, query.val[1])) ?? []).map(toWitFile) };
    case "contents":
      return { tag: "contents", val: toWitContents((await p.contents(ctx, remote, query.val[1], fromWitFile(query.val[2]))) ?? {}) };
    case "comments":
      return { tag: "comments", val: p.comments ? ((await p.comments(ctx, remote, query.val[1])) ?? []).map(toWitComment) : [] };
    case "add-comment":
      if (!p.addComment) throw CodeyardError.invalid(`the pull request provider '${name}' does not take comments; give it an addComment(ctx, remote, id, body) handler`);
      await p.addComment(ctx, remote, query.val[1], query.val[2]);
      return { tag: "done" };
    case "checkout":
      if (!p.checkout) throw CodeyardError.invalid(`the pull request provider '${name}' has no checkout handler; give it a checkout(ctx, remote, id) handler`);
      return { tag: "checkout", val: toWitCheckout(await p.checkout(ctx, remote, query.val[1])) };
  }
}
