// typescript.md, "fetch and npm packages".

import type { Context } from "@codeyard/extension";

declare const ctx: Context;

import { Octokit } from "@octokit/core";

const octokit = new Octokit({ request: { fetch: ctx.authFetch("github") } });
const { data } = await octokit.request("GET /user/repos");

export const repositories = data;
