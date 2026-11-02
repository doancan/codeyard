// GitLab: merge requests of GitLab repositories in the Pull Requests view, through the pull request provider API
// (`[[pull-requests]]` in the manifest, `ctx.pullRequests` here). Every request to GitLab goes through
// `ctx.fetchAs(remote.account, …)`: Codeyard signs it with the user's GitLab account, so the extension never sees the
// token, and only the hosts of the account's `[[oauth]]` table get it.
//
// The API is GitLab's REST API v4 on the remote's host, so the same code serves gitlab.com and the self-managed
// instances the user adds (README.md): a remote on an instance carries that instance's account, `gitlab@<host>`.

import type { ChangedFile, CheckState, Comment, Context, PullRequest, PullRequestProvider, Remote } from "@codeyard/extension";

// ---- GitLab's answers (the fields used here) ---------------------------------------------------------------------

interface MergeRequest {
  iid: number;
  title: string;
  description: string | null;
  state: "opened" | "closed" | "locked" | "merged";
  draft?: boolean;
  work_in_progress?: boolean;
  author: { username: string };
  source_branch: string;
  target_branch: string;
  updated_at: string;
  created_at: string;
  web_url: string;
  labels: string[];
  head_pipeline?: { id: number; status: string; web_url: string } | null;
  diff_refs?: { base_sha: string; head_sha: string; start_sha: string } | null;
}

interface Diff {
  old_path: string;
  new_path: string;
  new_file: boolean;
  renamed_file: boolean;
  deleted_file: boolean;
}

interface Note {
  body: string;
  system: boolean;
  created_at: string;
  author: { username: string };
  position?: { new_path?: string; new_line?: number | null } | null;
}

// ---- calls -------------------------------------------------------------------------------------------------------

/** `https://<host>/api/v4/projects/<group%2Fapp>`: GitLab takes the URL-encoded path where it takes a project id. */
export function projectUrl(remote: Remote): string {
  return `https://${remote.host}/api/v4/projects/${encodeURIComponent(remote.path)}`;
}

function accountOf(remote: Remote): string {
  if (!remote.account) throw new Error(`the [[pull-requests]] entry '${remote.provider}' needs auth = "<its [[oauth]] id>"`);
  return remote.account;
}

/** A signed request to GitLab; the body as JSON, or an error that says what GitLab answered. */
async function gitlab<T>(ctx: Context, remote: Remote, url: string, init: { method?: string; body?: string } = {}): Promise<T> {
  const account = accountOf(remote);
  // Asks the user to sign in the first time; returns at once after that.
  await ctx.session(account);
  const headers: Record<string, string> = { accept: "application/json" };
  if (init.body !== undefined) headers["content-type"] = "application/json";
  const res = await ctx.fetchAs(account, url, { ...init, headers });
  if (!res.ok) throw new Error(`${remote.host} answered ${res.status} for ${url.replace(/\?.*/, "")}${await reason(res)}`);
  return (await res.json()) as T;
}

/** GitLab's `message` or `error`, when the body has one. */
async function reason(res: { text(): Promise<string> }): Promise<string> {
  try {
    const body = JSON.parse(await res.text()) as { message?: unknown; error?: unknown };
    const m = body.message ?? body.error;
    return m ? `: ${typeof m === "string" ? m : JSON.stringify(m)}` : "";
  } catch {
    return "";
  }
}

const iid = (id: string) => {
  if (!/^\d+$/.test(id)) throw new Error(`'${id}' is not a merge request number`);
  return id;
};

const time = (s: string | undefined) => (s ? Date.parse(s) || undefined : undefined);

export function summary(mr: MergeRequest): PullRequest {
  return {
    id: String(mr.iid),
    reference: `!${mr.iid}`,
    title: mr.title,
    author: mr.author.username,
    state: mr.state === "merged" ? "merged" : mr.state === "opened" ? (mr.draft || mr.work_in_progress ? "draft" : "open") : "closed",
    sourceBranch: mr.source_branch,
    targetBranch: mr.target_branch,
    updated: time(mr.updated_at),
    url: mr.web_url,
  };
}

/** A pipeline status (`created`, `running`, `success`, …) as a check state. */
export function checkState(status: string): CheckState {
  switch (status) {
    case "success":
      return "success";
    case "failed":
      return "failure";
    case "canceled":
    case "skipped":
      return "neutral";
    default:
      return "pending";
  }
}

export function fileOf(d: Diff): ChangedFile {
  if (d.new_file) return { path: d.new_path, status: "added" };
  if (d.deleted_file) return { path: d.old_path, status: "deleted" };
  if (d.renamed_file) return { path: d.new_path, oldPath: d.old_path, status: "renamed" };
  return { path: d.new_path, status: "modified" };
}

/** A file's text at `sha`, or undefined when it is binary (has a NUL byte) or not there. */
async function fileAt(ctx: Context, remote: Remote, path: string, sha: string): Promise<string | undefined> {
  const account = accountOf(remote);
  const url = `${projectUrl(remote)}/repository/files/${encodeURIComponent(path)}/raw?ref=${encodeURIComponent(sha)}`;
  const res = await ctx.fetchAs(account, url);
  if (res.status === 404) return undefined;
  if (!res.ok) throw new Error(`${remote.host} answered ${res.status} for ${path} at ${sha.slice(0, 8)}`);
  const bytes = await res.bytes();
  if (bytes.includes(0)) return undefined;
  return new TextDecoder().decode(bytes);
}

export const provider: PullRequestProvider = {
  async list(ctx, remote) {
    const url = `${projectUrl(remote)}/merge_requests?state=opened&order_by=updated_at&sort=desc&per_page=100`;
    return (await gitlab<MergeRequest[]>(ctx, remote, url)).map(summary);
  },

  async details(ctx, remote, id) {
    const mr = await gitlab<MergeRequest>(ctx, remote, `${projectUrl(remote)}/merge_requests/${iid(id)}`);
    const p = mr.head_pipeline;
    return {
      summary: summary(mr),
      description: mr.description ?? "",
      checks: p ? [{ name: `Pipeline #${p.id}`, state: checkState(p.status), url: p.web_url }] : [],
      labels: mr.labels,
    };
  },

  async files(ctx, remote, id) {
    const diffs = await gitlab<Diff[]>(ctx, remote, `${projectUrl(remote)}/merge_requests/${iid(id)}/diffs?per_page=100`);
    return diffs.map(fileOf);
  },

  async contents(ctx, remote, id, file) {
    // The two sides of the diff GitLab shows: the merge base and the head of the source branch.
    const mr = await gitlab<MergeRequest>(ctx, remote, `${projectUrl(remote)}/merge_requests/${iid(id)}`);
    const refs = mr.diff_refs;
    if (!refs) return {};
    const [base, head] = await Promise.all([
      file.status === "added" ? undefined : fileAt(ctx, remote, file.oldPath ?? file.path, refs.base_sha),
      file.status === "deleted" ? undefined : fileAt(ctx, remote, file.path, refs.head_sha),
    ]);
    return { ...(base === undefined ? {} : { base }), ...(head === undefined ? {} : { head }) };
  },

  async comments(ctx, remote, id) {
    const url = `${projectUrl(remote)}/merge_requests/${iid(id)}/notes?sort=asc&order_by=created_at&per_page=100`;
    const notes = await gitlab<Note[]>(ctx, remote, url);
    return notes
      .filter((n) => !n.system)
      .map((n): Comment => {
        const c: Comment = { author: n.author.username, body: n.body, created: time(n.created_at) };
        if (n.position?.new_path) c.path = n.position.new_path;
        if (n.position?.new_line) c.line = n.position.new_line;
        return c;
      });
  },

  async addComment(ctx, remote, id, body) {
    await gitlab(ctx, remote, `${projectUrl(remote)}/merge_requests/${iid(id)}/notes`, { method: "POST", body: JSON.stringify({ body }) });
  },

  // GitLab keeps every merge request's head, forks included, at this ref of the target project.
  checkout: (_ctx, _remote, id) => ({ ref: `refs/merge-requests/${iid(id)}/head`, branch: `mr-${id}` }),
};

export function activate(ctx: Context) {
  ctx.pullRequests("gitlab", provider);
  ctx.command("sign-in", async (ctx) => {
    const account = await ctx.session("gitlab");
    ctx.notice(`GitLab: signed in as ${account.label}.`);
  });
}
