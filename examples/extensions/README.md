# Example extensions

Working Rust extensions built with the SDK (`crates/ext-sdk`, [docs/extensions/commands.md](../../docs/extensions/commands.md)). CI runs their unit tests and runs each one in `codeyard-exthost` the way `cyx test` does (`crates/cyx/src/code_tests.rs`).

| Example | Shows |
| --- | --- |
| `hello-command` | a command that asks for input, keeps a count in storage and shows a notice |
| `hello-view` | a sidebar view with a button, its events, and a command that resets it |

Each is exactly what `cyx new` writes, with the SDK taken from this checkout; a test keeps them in step with the templates in `crates/cyx/templates`. After changing a template, delete the example and write it again from this folder:

```sh
cyx new command hello-command --publisher codeyard --sdk ../../..
cyx new view hello-view --publisher codeyard --sdk ../../..
```

Run one:

```sh
cd hello-command
cargo test     # against the mock host
cyx test       # needs: rustup target add wasm32-wasip2
```
