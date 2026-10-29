// Tickets as Promises. A call that reaches the app returns a ticket at once; its result arrives later at the guest's
// `complete` export (streamed data before that at `event`), which resolves the Promise. In a component the engine runs
// the Promise reactions before each export returns, so an export never waits on I/O.

import { DeniedError, describe } from "./errors.ts";
import type { Bindings, Completion, StreamEvent, Ticket } from "./wit.ts";

interface Waiter {
  resolve(c: Completion): void;
  event?: (e: StreamEvent) => void;
}

let bindings: Bindings | undefined;
const waiting = new Map<Ticket, Waiter>();

/** Install the host's imports (the component's, or the mock host's). */
export function install(b: Bindings): void {
  bindings = b;
  waiting.clear();
}

export function host(): Bindings {
  if (!bindings) {
    throw new Error("the extension is not running in Codeyard: build it with `cyx package`, or test it with MockHost from @codeyard/extension/testing");
  }
  return bindings;
}

/**
 * An optional interface, or a refusal when the manifest does not grant it: the component then does not import it at
 * all (`cyx package` builds against the permissions the manifest declares).
 */
export function optional<K extends keyof Bindings>(key: K, line: () => string): NonNullable<Bindings[K]> {
  const b = host()[key];
  if (!b) {
    const e = new DeniedError(line(), "not-declared");
    // The host never sees this call, so the SDK reports it: `cyx test` fails on it as on any refusal.
    host().log.log("error", e.message);
    throw e;
  }
  return b as NonNullable<Bindings[K]>;
}

/** Wait for a ticket's completion; `event` receives its streamed data first. */
export function wait(ticket: Ticket, event?: (e: StreamEvent) => void): Promise<Completion> {
  return new Promise((resolve) => {
    waiting.set(ticket, { resolve, event });
  });
}

/** Start a call and wait for it; a refusal before the call (a missing import) rejects the same way the host would. */
export function call(start: () => Ticket, event?: (e: StreamEvent) => void): Promise<Completion> {
  try {
    return wait(start(), event);
  } catch (e) {
    return Promise.reject(e);
  }
}

/** `guest.complete`. */
export function complete(ticket: Ticket, c: Completion): void {
  const w = waiting.get(ticket);
  waiting.delete(ticket);
  w?.resolve(c);
}

/** `guest.event`. */
export function event(ticket: Ticket, e: StreamEvent): void {
  waiting.get(ticket)?.event?.(e);
}

/** Run work nobody waits for; a failure is logged with its stack. */
export function background(work: Promise<unknown> | (() => unknown)): void {
  let p: Promise<unknown>;
  try {
    p = Promise.resolve(typeof work === "function" ? work() : work);
  } catch (e) {
    p = Promise.reject(e);
  }
  p.catch((e) => {
    try {
      host().log.log("error", `a background task failed: ${describe(e)}`);
    } catch {
      // Not running in a host: nothing to report to.
    }
  });
}

/** Tickets still waiting; the mock host uses it to tell a stuck extension from a finished one. */
export function pendingTickets(): number {
  return waiting.size;
}
