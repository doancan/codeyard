# Doc snippets (TypeScript)

The TypeScript snippets of [docs/extensions](../../../docs/extensions), each copied here as written so CI type-checks
it (`npm run check`) and runs the tests among them (`npm test`). `crates/cyx/src/doc_tests.rs` fails when a snippet in
the docs is not found, line by line, in an example. Change a snippet in both places.

Each file under `src/` says which page its snippets come from. `src/extension.ts` is the command of
[typescript.md](../../../docs/extensions/typescript.md#your-first-command), which the tests in `test/` run.
