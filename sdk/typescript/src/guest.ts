// The guest's exports (`interface guest`), shared by the component and the mock host: activation, and each request
// routed to what the extension registered. `cyx` generates the component's entry module, which imports the extension
// and the interfaces its manifest grants and exports `guest = start(extension, bindings)`. Authors never call this.

import { Context } from "./context.ts";
import { CodeyardError, describe, toWit } from "./errors.ts";
import { fetch, FetchHeaders, FetchResponse } from "./fetch.ts";
import { handlers, resetHandlers } from "./handlers.ts";
import { answerPullRequests } from "./pulls.ts";
import { background, complete, event, host, install } from "./runtime.ts";
import { fromWitDiagnostic, toWitDiagnostic, type CodeAction } from "./types.ts";
import type { Bindings, Guest, Request, Response, WitCodeAction } from "./wit.ts";

/** An extension module: `export function activate(ctx) { … }`, optionally `export function deactivate(ctx) { … }`. */
export interface ExtensionModule {
  activate(ctx: Context): unknown;
  deactivate?(ctx: Context): unknown;
}

const ctx = new Context();

/** The storage key under which `ctx.state` waits for the next activation during a hot reload. Reserved. */
export const RELOAD_STATE_KEY = "codeyard.reload-state";

/** The activation reason of a hot reload (`cyx dev` swapped in a new build). */
export const RELOAD = "reload";

/** The snapshot is for the activation right after a reload; any other start forgets it. */
function restoreState(reason: string): void {
  const storage = host().storage;
  const saved = storage?.get(RELOAD_STATE_KEY);
  if (saved === undefined) return;
  storage.delete(RELOAD_STATE_KEY);
  if (reason !== RELOAD) return;
  try {
    const obj = JSON.parse(new TextDecoder().decode(saved)) as Record<string, unknown>;
    for (const [k, v] of Object.entries(obj)) handlers.state.set(k, v);
  } catch {
    // A snapshot that does not parse is dropped.
  }
}

function saveState(): void {
  if (handlers.state.size === 0) return;
  try {
    host().storage.set(RELOAD_STATE_KEY, new TextEncoder().encode(JSON.stringify(Object.fromEntries(handlers.state))));
  } catch (e) {
    host().log.log("warn", `the state was not kept for the reload: ${describe(e)}`);
  }
  handlers.state.clear();
}

/** Make `fetch` (and `Headers`, `Response` where the engine has none) the SDK's, mapped to the host's `net`. */
export function installGlobals(): () => void {
  const g = globalThis as Record<string, unknown>;
  const before = { fetch: g.fetch, Headers: g.Headers, Response: g.Response };
  const set = (name: string, value: unknown) => Object.defineProperty(g, name, { value, writable: true, configurable: true, enumerable: false });
  set("fetch", fetch);
  if (typeof g.Headers === "undefined") set("Headers", FetchHeaders);
  if (typeof g.Response === "undefined") set("Response", FetchResponse);
  return () => {
    for (const [k, v] of Object.entries(before)) set(k, v);
  };
}

/** The guest exports for `extension`, calling the host through `bindings`. */
export function start(extension: ExtensionModule, bindings: Bindings): Guest {
  if (typeof extension.activate !== "function") {
    throw new Error("the extension's entry module must export `function activate(ctx: Context)`");
  }
  install(bindings);
  resetHandlers();
  return {
    activate({ reason }) {
      handlers.reason = reason;
      restoreState(reason);
      let r: unknown;
      try {
        r = extension.activate(ctx);
      } catch (e) {
        // `result<_, string>`: a thrown string is the error.
        throw describe(e);
      }
      // `activate` may be async; what it awaits finishes in the background, and a failure is logged.
      if (r && typeof (r as Promise<unknown>).then === "function") background(r as Promise<unknown>);
    },
    deactivate() {
      try {
        const r = extension.deactivate?.(ctx);
        if (r && typeof (r as Promise<unknown>).then === "function") background(r as Promise<unknown>);
      } catch (e) {
        host().log.log("error", `deactivate failed: ${describe(e)}`);
      }
      saveState();
    },
    handle(id, request) {
      handle(id, request);
    },
    cancel(id) {
      handlers.cancelledRequests.add(id);
    },
    complete(ticket, result) {
      complete(ticket, result);
    },
    event(ticket, e) {
      event(ticket, e);
    },
  };
}

function respond(id: bigint, result: { tag: "ok"; val: Response } | { tag: "err"; val: ReturnType<typeof toWit> }): void {
  if (handlers.cancelledRequests.delete(id)) return;
  host().requests.respond(id, result);
}

const none: Response = { tag: "none" };

/** Answer request `id` with what `work` returns, mapped to a response; a throw or rejection answers with the error. */
function answer<T>(id: bigint, work: () => T | Promise<T>, map: (v: T) => Response): void {
  Promise.resolve()
    .then(work)
    .then(
      (v) => respond(id, { tag: "ok", val: map(v) }),
      (e) => respond(id, { tag: "err", val: toWit(e) }),
    )
    .catch((e) => respond(id, { tag: "err", val: toWit(e) }));
}

function json(v: unknown): string {
  return JSON.stringify(v === undefined ? null : v);
}

function codeAction(a: CodeAction): WitCodeAction {
  return {
    id: a.id,
    title: a.title,
    edits: Object.entries(a.edits ?? {}),
    command: a.command,
    preferred: a.preferred ?? false,
    fixes: (a.fixes ?? []).map(toWitDiagnostic),
  };
}

function handle(id: bigint, request: Request): void {
  const missing = (what: string) => respond(id, { tag: "err", val: toWit(CodeyardError.notFound(what)) });
  switch (request.tag) {
    case "command": {
      const [command, argsJson] = request.val;
      const f = handlers.commands.get(command);
      if (!f) return missing(`no command '${command}' is registered; call ctx.command("${command}", …) in activate`);
      let args: unknown;
      try {
        args = argsJson.trim() ? JSON.parse(argsJson) : null;
      } catch (e) {
        return respond(id, { tag: "err", val: toWit(CodeyardError.invalid(`the arguments of ${command} are not JSON: ${(e as Error).message}`)) });
      }
      return answer(id, () => f(ctx, args), (v) => ({ tag: "json", val: json(v) }));
    }
    case "render": {
      const f = handlers.views.get(request.val);
      if (!f) return missing(`no view '${request.val}' is registered; call ctx.view("${request.val}", …) in activate`);
      return answer(id, () => f(ctx), (tree) => ({ tag: "view", val: json(tree) }));
    }
    case "ui-event": {
      const [view, eventJson] = request.val;
      const f = handlers.viewEvents.get(view);
      if (!f) return respond(id, { tag: "ok", val: none });
      let e: unknown;
      try {
        e = JSON.parse(eventJson);
      } catch (err) {
        return respond(id, { tag: "err", val: toWit(CodeyardError.invalid(`the event is not JSON: ${(err as Error).message}`)) });
      }
      return answer(id, () => f(ctx, e), () => none);
    }
    case "format": {
      const f = handlers.formatter;
      if (!f) return missing("no formatter is registered; call ctx.formatter(…) in activate");
      const [document, range, options] = request.val;
      return answer(id, () => f(ctx, document, range, options) as never, (edits: unknown) => ({ tag: "edits", val: edits as [] }));
    }
    case "code-actions": {
      const f = handlers.codeActions;
      if (!f) return respond(id, { tag: "ok", val: { tag: "code-actions", val: [] } });
      const [document, range, diagnostics] = request.val;
      return answer(
        id,
        () => f(ctx, document, range, diagnostics.map(fromWitDiagnostic)) as CodeAction[],
        (actions) => ({ tag: "code-actions", val: (actions ?? []).map(codeAction) }),
      );
    }
    case "run-code-action": {
      const f = handlers.runCodeAction;
      if (!f) return missing("no code action handler is registered; call ctx.onCodeAction(…) in activate");
      return answer(id, () => f(ctx, request.val), () => none);
    }
    case "open-uri": {
      const f = handlers.openUri;
      if (!f) return respond(id, { tag: "ok", val: none });
      return answer(id, () => f(ctx, request.val), () => none);
    }
    case "pull-requests":
      return answer(id, () => answerPullRequests(ctx, request.val), (val) => ({ tag: "pull-requests", val }));
    case "settings-changed": {
      // Answered at once; the handlers run in the background.
      respond(id, { tag: "ok", val: none });
      for (const f of handlers.settingsChanged) background(() => f(ctx, request.val));
      return;
    }
    case "timer": {
      respond(id, { tag: "ok", val: none });
      const f = handlers.timers.get(request.val);
      if (f) background(() => f(ctx));
      return;
    }
    case "cancel-progress": {
      handlers.cancelledProgress.add(request.val);
      return respond(id, { tag: "ok", val: none });
    }
    case "provide-tasks":
    case "run-task":
      return respond(id, { tag: "err", val: toWit(CodeyardError.invalid("this SDK does not provide tasks yet")) });
  }
}
