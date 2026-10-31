// TITLE: pull requests of a forge in Codeyard's Pull Requests view. It speaks the Gitea and Forgejo API (`/api/v1`);
// for another forge, change `api` and the fields read below. Every call goes through `ctx.fetchAs` with the remote's
// account: Codeyard adds the token, so the extension never sees it. `npm test` runs test/extension.test.ts against a
// mock Codeyard with HTTP fixtures; `cyx test` runs the built extension with the answers in test/fixtures.json.

import type { ChangedFile, Context, PullRequest, PullRequestProvider, Remote } from "@codeyard/extension";

/** `https://<the remote's host>/api/v1/repos/<owner>/<repo><rest>`: the same code serves every host it claims. */
function api(remote: Remote, rest: string): string {
  return `https://${remote.host}/api/v1/repos/${remote.path}${rest}`;
}

/** GET as the remote's account; the first call asks the user to sign in. */
async function get<T>(ctx: Context, remote: Remote, rest: string): Promise<T> {
  if (!remote.account) throw new Error(`the [[pull-requests]] entry '${remote.provider}' needs auth = "<its [[oauth]] id>"`);
  await ctx.session(remote.account);
  const res = await ctx.fetchAs(remote.account, api(remote, rest), { headers: { accept: "application/json" } });
  if (!res.ok) throw new Error(`${remote.host} answered ${res.status} for ${rest}`);
  return (await res.json()) as T;
}

/** A pull request as the API returns it (the fields used here). */
interface Pull {
  number: number;
  title: string;
  state: string;
  merged?: boolean;
  draft?: boolean;
  user: { login: string };
  head: { ref: string };
  base: { ref: string };
  html_url?: string;
  body?: string;
  labels?: { name: string }[];
}

export function summary(p: Pull): PullRequest {
  return {
    id: String(p.number),
    reference: `#${p.number}`,
    title: p.title,
    author: p.user.login,
    state: p.merged ? "merged" : p.state === "closed" ? "closed" : p.draft ? "draft" : "open",
    sourceBranch: p.head.ref,
    targetBranch: p.base.ref,
    url: p.html_url,
  };
}

const provider: PullRequestProvider = {
  async list(ctx, remote) {
    return (await get<Pull[]>(ctx, remote, "/pulls?state=open&sort=recentupdate&limit=50")).map(summary);
  },

  async details(ctx, remote, id) {
    const p = await get<Pull>(ctx, remote, `/pulls/${id}`);
    return { summary: summary(p), description: p.body ?? "", labels: (p.labels ?? []).map((l) => l.name) };
  },

  async files(ctx, remote, id) {
    const files = await get<{ filename: string; previous_filename?: string; status: string }[]>(ctx, remote, `/pulls/${id}/files`);
    return files.map(
      (f): ChangedFile => ({
        path: f.filename,
        oldPath: f.previous_filename || undefined,
        status: f.status === "added" ? "added" : f.status === "deleted" || f.status === "removed" ? "deleted" : f.status === "renamed" ? "renamed" : "modified",
      }),
    );
  },

  // The diff shows nothing until this fetches the file at both ends (`/raw/<path>?ref=<sha>` here).
  contents: () => ({}),

  // Gitea and Forgejo keep every pull request's head at `refs/pull/<n>/head`, forks included.
  checkout: (_ctx, _remote, id) => ({ ref: `refs/pull/${id}/head`, branch: `pr-${id}` }),
};

export function activate(ctx: Context) {
  // `pulls` is the [[pull-requests]] id in codeyard-extension.toml.
  ctx.pullRequests("pulls", provider);
}
