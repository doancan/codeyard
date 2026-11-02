# Hello View Ts

A Codeyard extension in TypeScript. You need Node.js 22.18 or newer; `cyx` installs its build tools (esbuild, ComponentizeJS) itself.

```sh
npm install    # the SDK, its types and TypeScript
npm test       # unit tests, against a mock Codeyard
npm run check  # type-check
cyx test       # the built extension, in the real extension host
cyx dev        # run it in Codeyard
cyx package    # build extension.wasm and write the .cyx
```

Publish it with `cyx publish` or by pushing a tag (`git tag v0.1.0 && git push --tags`).
