// What the extension registered in `activate`.

import type { Context } from "./context.ts";
import type { PullRequestProvider } from "./pulls.ts";

type Fn = (ctx: Context, ...args: any[]) => unknown;

export interface Handlers {
  commands: Map<string, Fn>;
  views: Map<string, Fn>;
  viewEvents: Map<string, Fn>;
  formatter: Fn | undefined;
  codeActions: Fn | undefined;
  runCodeAction: Fn | undefined;
  openUri: Fn | undefined;
  hover: Fn | undefined;
  completion: Fn | undefined;
  codeLenses: Fn | undefined;
  resolveCodeLens: Fn | undefined;
  inlayHints: Fn | undefined;
  /** `ctx.pullRequests`: by the `[[pull-requests]]` local id. */
  pullRequests: Map<string, PullRequestProvider>;
  settingsChanged: Fn[];
  timers: Map<bigint, Fn>;
  /** `ctx.onDocuments` and `ctx.onFiles` watches, by id. */
  documentWatches: Map<bigint, Fn>;
  fileWatches: Map<bigint, Fn>;
  /** Progress the user cancelled. */
  cancelledProgress: Set<bigint>;
  /** Requests the host stopped waiting for. */
  cancelledRequests: Set<bigint>;
  reason: string;
  /** `ctx.state`: kept across a hot reload in `cyx dev` (`RELOAD_STATE_KEY`). */
  state: Map<string, unknown>;
  /** Source control providers, by local id. */
  scmStatus: Map<string, Fn>;
  scmOriginal: Map<string, Fn>;
  scmValidate: Map<string, Fn>;
}

function empty(): Handlers {
  return {
    commands: new Map(),
    views: new Map(),
    viewEvents: new Map(),
    formatter: undefined,
    codeActions: undefined,
    runCodeAction: undefined,
    openUri: undefined,
    hover: undefined,
    completion: undefined,
    codeLenses: undefined,
    resolveCodeLens: undefined,
    inlayHints: undefined,
    pullRequests: new Map(),
    settingsChanged: [],
    timers: new Map(),
    documentWatches: new Map(),
    fileWatches: new Map(),
    cancelledProgress: new Set(),
    cancelledRequests: new Set(),
    reason: "",
    state: new Map(),
    scmStatus: new Map(),
    scmOriginal: new Map(),
    scmValidate: new Map(),
  };
}

export const handlers: Handlers = empty();

/** Forget everything (a new mock host, or a reload). */
export function resetHandlers(): void {
  Object.assign(handlers, empty());
}
