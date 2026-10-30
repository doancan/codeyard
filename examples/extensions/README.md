# Example extensions

Working extensions built with the SDKs: Rust (`crates/ext-sdk`, [docs/extensions/commands.md](../../docs/extensions/commands.md)) and TypeScript (`sdk/typescript`, [docs/extensions/typescript.md](../../docs/extensions/typescript.md)). CI runs their unit tests and runs each one in `codeyard-exthost` the way `cyx test` does (`crates/cyx/src/code_tests.rs`).

| Example | Shows |
| --- | --- |
| `hello-command` | Rust: a command that asks for input, keeps a count in storage and shows a notice |
| `hello-view` | Rust: a sidebar view with a button, its events, and a command that resets it |
| `hello-command-ts` | TypeScript: the same command |
| `hello-view-ts` | TypeScript: the same view |
| `octokit-repos-ts` | TypeScript with npm packages: Octokit lists your GitHub repositories through `auth`, zod checks the answer |

The `hello-*` examples are exactly what `cyx new` writes, with the SDK taken from this checkout; a test keeps them in step with the templates in `crates/cyx/templates`. After changing a template, delete the example and write it again from this folder:

```sh
cyx new command hello-command --publisher codeyard --sdk ../../..
cyx new view hello-view --publisher codeyard --sdk ../../..
cyx new command hello-command-ts --lang ts --publisher codeyard --sdk ../../..
cyx new view hello-view-ts --lang ts --publisher codeyard --sdk ../../..
```

Then run `npm install --before=<a date two weeks ago>` in each TypeScript one, so its lock file pins versions that have been public for at least two weeks.

Run one:

```sh
cd hello-command
cargo test     # against the mock host
cyx test       # needs: rustup target add wasm32-wasip2

cd hello-command-ts
npm install
npm test       # against the mock host
cyx test       # needs: Node.js 22.18 or newer
```
