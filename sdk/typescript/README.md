# @codeyard/extension

Write Codeyard extensions in TypeScript. The guide is [docs/extensions/typescript.md](../../docs/extensions/typescript.md).

```ts
import type { Context } from "@codeyard/extension";

export function activate(ctx: Context) {
  ctx.command("hello", async (ctx) => {
    const name = (await ctx.input({ prompt: "Your name?" })) || "world";
    ctx.notice(`Hello, ${name}!`);
  });
}
```

| Path | What it is |
| --- | --- |
| `src/index.ts` | The public API: `Context`, `DeniedError`, `CodeyardError`, `fetch` and the types |
| `src/guest.ts`, `src/runtime.ts` | The component's exports (`activate`, `handle`, `cancel`, `complete`, `event`, `deactivate`) and the ticket → Promise table |
| `src/wit.ts` | The `codeyard:api` WIT as ComponentizeJS shows it to JavaScript |
| `src/testing/` | `@codeyard/extension/testing`: the mock host for `npm test` |
| `toolchain/` | What `cyx` installs in its cache folder to build TypeScript extensions: esbuild and ComponentizeJS at pinned versions (`package-lock.json`), and `build.mjs` |
| `test/` | The SDK's own tests |

`cyx` embeds `src/` (except `testing/`) and `toolchain/` and bundles that copy into every extension, so the glue always matches the API the extension host implements. The package ships TypeScript sources; Node.js 22.18 or newer runs them directly in tests. Publishing to npm will need a step that compiles them to JavaScript, since Node.js does not strip types inside `node_modules`.

```sh
npm ci
npm run check   # tsc
npm test
```

The package has no runtime dependencies. Its development dependencies (TypeScript, Node.js types) and the build tools in `toolchain/` are not shipped in any extension, except ComponentizeJS's JavaScript engine (StarlingMonkey, Apache-2.0 WITH LLVM-exception), which every TypeScript extension's component embeds. `THIRD_PARTY_NPM.md` in the repository root lists them with their licences.
