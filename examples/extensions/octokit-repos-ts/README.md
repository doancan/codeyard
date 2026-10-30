# GitHub Repos

A Codeyard extension in TypeScript that uses plain npm packages: [Octokit](https://github.com/octokit/core.js) lists
your GitHub repositories, [zod](https://zod.dev) checks the answer, and a quick pick opens the one you choose. Codeyard
signs the requests with your GitHub account (`auth = ["github"]`); the extension never sees the token.

```sh
npm install    # Octokit, zod, the SDK, its types and TypeScript
npm test       # unit tests against a mock Codeyard, with fixture responses: no network
npm run check  # type-check
cyx test       # the built extension in the real extension host (which has no network either)
cyx dev        # run it in Codeyard
```

`cyx package` bundles Octokit and zod into `extension.wasm` with esbuild. Packages that need Node.js built-ins (`fs`,
`child_process`, `net`) fail at that step with the import that needs them.
