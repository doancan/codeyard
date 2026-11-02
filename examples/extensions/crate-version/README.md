# Crate Version

A command that asks for a Rust crate's name and shows its latest version from crates.io. It shows `fetch` and the `net` permission; the unit test answers the request with a fixture.

```sh
cargo test     # unit tests, against a mock Codeyard
cyx test       # the built extension, in the real extension host
cyx dev        # run it in Codeyard; saving a file rebuilds and reloads it
```

You need Rust and its WebAssembly target: `rustup target add wasm32-wasip2`.
