# TODO Actions

Code actions for TODO comments in Rust and Markdown: one edits the document (mark it done), the other runs a command of the extension (copy the line into a list kept in storage). Like formatters, code actions get their document from Codeyard and need no permission.

```sh
cargo test     # unit tests, against a mock Codeyard
cyx test       # the built extension, in the real extension host
cyx dev        # run it in Codeyard; saving a file rebuilds and reloads it
```

You need Rust and its WebAssembly target: `rustup target add wasm32-wasip2`.
