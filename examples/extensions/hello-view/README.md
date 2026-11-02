# Hello View

A Codeyard extension in Rust. You need Rust and its WebAssembly target: `rustup target add wasm32-wasip2`.

```sh
cargo test     # unit tests, against a mock Codeyard
cyx test       # the built extension, in the real extension host
cyx dev        # run it in Codeyard
cyx package    # build extension.wasm and write the .cyx
```

Publish it with `cyx publish` or by pushing a tag (`git tag v0.1.0 && git push --tags`).
