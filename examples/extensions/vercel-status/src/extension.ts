// Vercel Status: the state of the linked project's latest deployment in the status bar, its recent deployments in a
// sidebar view, and a command to refresh both. Requests go to api.vercel.com with the user's access token, which
// Vercel Status keeps in the keychain (`secrets`); the token never leaves for any other host.
//
// The project is the one `vercel link` wrote to .vercel/project.json, or the `project` setting.

import type { Context, Element, StatusItem, Tone } from "@codeyard/extension";

const API = "https://api.vercel.com";
const TOKEN = "token";
const VIEW = "deployments";
const STATUS = "deployment";

/** A deployment, as `GET /v6/deployments` lists it (the fields used here). */
export interface Deployment {
  uid: string;
  name: string;
  url: string;
  created: number;
  state?: string;
  readyState?: string;
  target?: string | null;
  inspectorUrl?: string | null;
  meta?: { githubCommitRef?: string; githubCommitMessage?: string; gitlabCommitRef?: string; gitlabCommitMessage?: string };
}

/** The project to ask about: an id or name, and the team that owns it when it is not a personal project. */
export interface Project {
  id: string;
  team?: string;
}

/** What the extension knows after its last refresh. */
type Snapshot =
  | { kind: "no-token" }
  | { kind: "no-project" }
  | { kind: "error"; message: string; project: Project }
  | { kind: "ok"; project: Project; deployments: Deployment[] };

let snapshot: Snapshot | undefined;
let refreshing: Promise<void> | undefined;
let timer: { clear(): void } | undefined;

export function activate(ctx: Context) {
  ctx.command("refresh", async (ctx) => {
    await refresh(ctx);
    if (snapshot?.kind === "error") ctx.notice(snapshot.message);
  });
  ctx.command("set-token", setToken);
  ctx.command("forget-token", async (ctx) => {
    await ctx.deleteSecret(TOKEN);
    ctx.notice("Vercel Status forgot the access token.");
    await refresh(ctx);
  });

  ctx.view(VIEW, (_ctx) => view(snapshot));
  ctx.onViewEvent(VIEW, async (ctx, event) => {
    const clicked = event?.click?.path?.[0];
    if (clicked === "refresh") await refresh(ctx);
    if (clicked === "set-token") await setToken(ctx);
  });

  ctx.onSettingsChanged(async (ctx, keys) => {
    if (keys.includes("interval")) startTimer(ctx);
    if (keys.includes("project") || keys.includes("team")) await refresh(ctx);
  });
  startTimer(ctx);
  ctx.background(refresh(ctx));
}

export function deactivate() {
  timer?.clear();
  timer = undefined;
  snapshot = undefined;
  refreshing = undefined;
}

function startTimer(ctx: Context) {
  timer?.clear();
  const seconds = Math.max(30, Number(ctx.setting<number>("interval")) || 60);
  timer = ctx.every(seconds, refresh);
}

/** Ask Vercel again, then update the status item and the view. One refresh at a time. */
function refresh(ctx: Context): Promise<void> {
  refreshing ??= load(ctx)
    .then((s) => {
      snapshot = s;
      showStatus(ctx, s);
      ctx.invalidate(VIEW);
    })
    .finally(() => {
      refreshing = undefined;
    });
  return refreshing;
}

async function load(ctx: Context): Promise<Snapshot> {
  const token = await ctx.secret(TOKEN);
  if (!token) return { kind: "no-token" };
  const project = await findProject(ctx);
  if (!project) return { kind: "no-project" };
  try {
    return { kind: "ok", project, deployments: await deployments(token, project) };
  } catch (e) {
    ctx.log("warn", `listing deployments failed: ${(e as Error).message}`);
    return { kind: "error", message: (e as Error).message, project };
  }
}

/** The `project` setting, or the first workspace folder linked with `vercel link`. */
export async function findProject(ctx: Context): Promise<Project | undefined> {
  const team = ctx.setting<string>("team")?.trim() || undefined;
  const id = ctx.setting<string>("project")?.trim();
  if (id) return { id, team };
  for (const folder of ctx.folders()) {
    let text: string;
    try {
      text = await ctx.readText(`${folder.replace(/\/$/, "")}/.vercel/project.json`);
    } catch {
      continue;
    }
    try {
      const linked = JSON.parse(text) as { projectId?: string; orgId?: string };
      if (!linked.projectId) continue;
      // A personal project's orgId is the user's id, which `teamId` does not take.
      const org = linked.orgId?.startsWith("team_") ? linked.orgId : undefined;
      return { id: linked.projectId, team: team ?? org };
    } catch {
      ctx.log("warn", `${folder}/.vercel/project.json is not JSON`);
    }
  }
  return undefined;
}

/** The project's latest deployments, newest first. */
export async function deployments(token: string, project: Project, limit = 20): Promise<Deployment[]> {
  let url = `${API}/v6/deployments?projectId=${encodeURIComponent(project.id)}&limit=${limit}`;
  if (project.team) url += `&teamId=${encodeURIComponent(project.team)}`;
  const res = await fetch(url, { headers: { authorization: `Bearer ${token}` } });
  if (res.status === 401 || res.status === 403) {
    throw new Error(`Vercel refused the access token (${res.status}). Run Vercel: Set Access Token.`);
  }
  if (res.status === 404) throw new Error(`Vercel has no project ${project.id} for this token.`);
  if (!res.ok) throw new Error(`Vercel answered ${res.status}.`);
  const body = (await res.json()) as { deployments?: Deployment[] };
  return body.deployments ?? [];
}

async function setToken(ctx: Context): Promise<void> {
  const token = (
    await ctx.input({
      title: "Vercel Access Token",
      prompt: "Create one at vercel.com/account/tokens. It is kept in your keychain and sent only to api.vercel.com.",
      password: true,
    })
  )?.trim();
  if (!token) return;
  const res = await fetch(`${API}/v2/user`, { headers: { authorization: `Bearer ${token}` } });
  if (!res.ok) {
    ctx.notice(`Vercel did not accept that token (${res.status}); it was not saved.`);
    return;
  }
  const { user } = (await res.json()) as { user?: { username?: string } };
  await ctx.setSecret(TOKEN, token);
  ctx.notice(`Vercel Status is signed in as ${user?.username ?? "you"}.`);
  await refresh(ctx);
}

// ---- what the user sees ------------------------------------------------------------------------------------------

/** Vercel's states, in words and tones. */
export function stateOf(d: Deployment): { label: string; tone: Tone } {
  switch ((d.state ?? d.readyState ?? "").toUpperCase()) {
    case "READY":
      return { label: "Ready", tone: "success" };
    case "BUILDING":
      return { label: "Building", tone: "warning" };
    case "INITIALIZING":
    case "QUEUED":
      return { label: "Queued", tone: "warning" };
    case "ERROR":
      return { label: "Error", tone: "danger" };
    case "CANCELED":
      return { label: "Canceled", tone: "muted" };
    default:
      return { label: "Unknown", tone: "muted" };
  }
}

const branchOf = (d: Deployment) => d.meta?.githubCommitRef ?? d.meta?.gitlabCommitRef ?? "";
const messageOf = (d: Deployment) => (d.meta?.githubCommitMessage ?? d.meta?.gitlabCommitMessage ?? "").split("\n")[0] ?? "";

/** `2026-10-31 14:05 UTC`: the same for every user, unlike a relative time that would need redrawing. */
export function when(ms: number): string {
  return `${new Date(ms).toISOString().slice(0, 16).replace("T", " ")} UTC`;
}

function showStatus(ctx: Context, s: Snapshot) {
  const item: StatusItem = { id: STATUS, text: "Vercel", tone: "muted" };
  if (s.kind === "no-token") {
    item.tooltip = "Vercel Status: no access token. Click to set one.";
    item.command = "set-token";
  } else if (s.kind === "no-project") {
    item.tooltip = "Vercel Status: no linked project. Run `vercel link`, or set the project in Settings.";
  } else if (s.kind === "error") {
    item.text = "Vercel: ?";
    item.tone = "danger";
    item.tooltip = s.message;
  } else if (s.deployments.length === 0) {
    item.text = "Vercel: no deployments";
  } else {
    const latest = s.deployments[0]!;
    const { label, tone } = stateOf(latest);
    item.text = `Vercel: ${label}`;
    item.tone = tone;
    item.tooltip = [latest.name, branchOf(latest), when(latest.created), `https://${latest.url}`].filter(Boolean).join(" · ");
  }
  ctx.setStatusItem(item);
}

/**
 * A node of the view, in the JSON form Codeyard draws (docs/extensions/reference/ui.md). Not the `{ type: … }`
 * shorthand of the `cyx new view` template, which Codeyard refuses (#182); hence the cast to `Element`.
 */
function node(kind: string, props: Record<string, unknown>, key?: string, children?: Element[]): Element {
  const n: Record<string, unknown> = { element: { [kind]: props } };
  if (key !== undefined) n.key = key;
  if (children?.length) n.children = children;
  return n as unknown as Element;
}

export function view(s: Snapshot | undefined): Element {
  const refresh = node("button", { label: "Refresh", icon: "refresh" }, "refresh");
  if (!s) return node("column", { pad: 2 }, undefined, [node("progress", { label: "Loading deployments" })]);
  if (s.kind === "no-token") {
    return node(
      "empty-state",
      { title: "Connect to Vercel", text: "Set an access token to see this project's deployments.", icon: "key" },
      undefined,
      [node("button", { label: "Set Access Token", tone: "accent" }, "set-token")],
    );
  }
  if (s.kind === "no-project") {
    return node(
      "empty-state",
      { title: "No linked project", text: "Run `vercel link` in this folder, or set Vercel Status › Project in Settings." },
      undefined,
      [refresh],
    );
  }
  const top = [node("row", { gap: 2 }, undefined, [node("text", { text: s.project.id, tone: "muted" }), refresh])];
  if (s.kind === "error") return node("column", { gap: 2, pad: 2 }, undefined, [node("banner", { text: s.message, tone: "danger" }), ...top]);
  if (s.deployments.length === 0) return node("column", { gap: 2, pad: 2 }, undefined, [...top, node("text", { text: "No deployments yet.", tone: "muted" })]);
  const rows = s.deployments.map((d) => {
    const { label, tone } = stateOf(d);
    return node("row", {}, d.uid, [
      node("badge", { text: label, tone }),
      node("text", { text: d.target === "production" ? "Production" : "Preview" }),
      node("text", { text: branchOf(d) || "—" }),
      node("text", { text: messageOf(d) || d.name }),
      node("text", { text: when(d.created), tone: "muted" }),
      node("link", { label: "Open", url: `https://${d.url}` }, "open"),
    ]);
  });
  const table = node("table", { label: "Deployments", columns: ["State", "Target", "Branch", "Commit", "Created", "Link"] }, "deployments", rows);
  return node("column", { gap: 2, pad: 2 }, undefined, [...top, table]);
}
