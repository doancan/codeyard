// What the extension registered in `activate`.

import type { Context } from "./context.ts";

type Fn = (ctx: Context, ...args: any[]) => unknown;

export interface Handlers {
  commands: Map<string, Fn>;
  views: Map<string, Fn>;
  viewEvents: Map<string, Fn>;
  formatter: Fn | undefined;
  codeActions: Fn | undefined;
  runCodeAction: Fn | undefined;
  openUri: Fn | undefined;
  settingsChanged: Fn[];
  timers: Map<bigint, Fn>;
  /** Progress the user cancelled. */
  cancelledProgress: Set<bigint>;
  /** Requests the host stopped waiting for. */
  cancelledRequests: Set<bigint>;
  reason: string;
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
    settingsChanged: [],
    timers: new Map(),
    cancelledProgress: new Set(),
    cancelledRequests: new Set(),
    reason: "",
  };
}

export const handlers: Handlers = empty();

/** Forget everything (a new mock host, or a reload). */
export function resetHandlers(): void {
  Object.assign(handlers, empty());
}
