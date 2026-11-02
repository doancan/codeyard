// GitHub Repos: lists the user's repositories with Octokit, signed for their GitHub account through `auth`, checks the
// answer with zod, and opens the one they pick. Plain npm packages, bundled by `cyx package`; `fetch` reaches GitHub
// through Codeyard.

import type { Context } from "@codeyard/extension";
import { Octokit } from "@octokit/core";
import { z } from "zod";

const Repos = z.array(
  z.object({
    full_name: z.string(),
    html_url: z.string().url(),
    description: z.string().nullable(),
  }),
);

export function activate(ctx: Context) {
  ctx.command("list", async (ctx) => {
    // The first use asks the user to sign in; after that Codeyard keeps the session.
    await ctx.session("github", ["repo"]);
    const octokit = new Octokit({ userAgent: "codeyard-github-repos", request: { fetch: ctx.authFetch("github") } });
    const { data } = await octokit.request("GET /user/repos", { per_page: 100, sort: "updated" });
    const repos = Repos.parse(data);
    const items = repos.map((r) => ({ label: r.full_name, detail: r.description ?? undefined }));
    const picked = await ctx.quickPick(items, { placeholder: "Open a repository" });
    if (picked !== undefined) await ctx.openExternal(repos[picked]!.html_url);
    return repos.map((r) => r.full_name);
  });
}
