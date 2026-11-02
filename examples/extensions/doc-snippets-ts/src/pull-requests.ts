// typescript.md, "Pull request providers".

import type { Context, PullRequest, Remote } from "@codeyard/extension";

/** One request, as `list` maps them. */
declare function pull(ctx: Context, remote: Remote, id: string): Promise<PullRequest>;

export function activate(ctx: Context) {
  ctx.pullRequests("gitea", {
    async list(ctx, remote) {
      const r = await ctx.fetchAs(remote.account!, `https://${remote.host}/api/v1/repos/${remote.path}/pulls?state=open`);
      return (await r.json()).map((p: any) => ({
        id: String(p.number),
        reference: `#${p.number}`,
        title: p.title,
        author: p.user.login,
        state: p.draft ? "draft" : "open",
        sourceBranch: p.head.ref,
        targetBranch: p.base.ref,
        updated: Date.parse(p.updated_at),
        url: p.html_url,
      }));
    },
    details: async (ctx, remote, id) => ({ summary: await pull(ctx, remote, id), description: "…", checks: [], labels: [] }),
    files: async (ctx, remote, id) => [{ path: "src/a.ts", status: "modified" }],
    contents: async (ctx, remote, id, file) => ({ base: "…", head: "…" }),
    comments: async (ctx, remote, id) => [],
    addComment: async (ctx, remote, id, body) => {},
    checkout: async (ctx, remote, id) => ({ ref: `refs/pull/${id}/head`, branch: `pr-${id}` }),
  });
}
