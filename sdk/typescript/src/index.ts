// `@codeyard/extension`: write Codeyard extensions in TypeScript (docs/extensions/typescript.md).
//
//   import type { Context } from "@codeyard/extension";
//
//   export function activate(ctx: Context) {
//     ctx.command("greet", async (ctx) => {
//       const name = await ctx.input({ prompt: "Your name?" });
//       if (name) ctx.notice(`Hello, ${name}!`);
//     });
//   }
//
// `cyx package` bundles the extension with esbuild and builds a WebAssembly component with ComponentizeJS. Calls that
// reach Codeyard return Promises: each call returns a ticket at once, and the Promise settles when the host completes
// it, so an export never waits on I/O. The global `fetch` is mapped to the host's `net`. Under Node, the mock host of
// `@codeyard/extension/testing` runs the same code for `npm test`.
//
// Design: docs/plugin-architecture.md §5.2.

export { Context, Process, Progress, Storage, Timer } from "./context.ts";
export { CodeyardError, DeniedError, type ErrorKind } from "./errors.ts";
export { fetch, FetchHeaders, FetchResponse, hostOf, type FetchInit } from "./fetch.ts";
export type { ExtensionModule } from "./guest.ts";
export * from "./types.ts";
