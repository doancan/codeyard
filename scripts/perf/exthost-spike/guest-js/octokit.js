// A realistic integration extension: Octokit and zod loaded and used at activation (no request is made).
import { log } from "codeyard:spike/host@0.1.0";
import { Octokit } from "@octokit/core";
import { z } from "zod";

const Deploy = z.object({ id: z.number(), state: z.enum(["ok", "failed"]), url: z.string().url() });
let octokit;

export function activate() {
  octokit = new Octokit({ userAgent: "codeyard-spike" });
  log("activated");
}

export function call(input) {
  const parsed = Deploy.safeParse({ id: input.length, state: "ok", url: `https://example.com/${input}` });
  return `${typeof octokit.request} ${parsed.success}`;
}
