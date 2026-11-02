# Branch Status

A command that runs `git` in the workspace and shows the current branch in the status bar. It shows `run`: running a program is high risk, so the manifest says why.

```sh
cargo test     # unit tests, against a mock Codeyard
cyx test       # the built extension, in the real extension host
cyx dev        # run it in Codeyard; saving a file rebuilds and reloads it
```

You need Rust and its WebAssembly target: `rustup target add wasm32-wasip2`.
