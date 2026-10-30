# Crate Version Ts

The `crate-version` example in TypeScript: a command that asks for a Rust crate's name and shows its latest version from crates.io, with the global `fetch` and the `net` permission. The unit test answers the request with a fixture.

```sh
npm install
npm test       # unit tests, against a mock Codeyard
cyx test       # the built extension, in the real extension host (needs Node.js 22.18 or newer)
cyx dev        # run it in Codeyard; saving a file rebuilds and reloads it
```
