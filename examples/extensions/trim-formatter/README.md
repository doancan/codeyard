# Trim Formatter

A formatter for Markdown and plain text: it removes spaces at the ends of lines and makes the file end with one newline. A formatter gets its document from Codeyard, so it needs no permission.

```sh
cargo test     # unit tests, against a mock Codeyard
cyx test       # the built extension, in the real extension host
cyx dev        # run it in Codeyard; saving a file rebuilds and reloads it
```

You need Rust and its WebAssembly target: `rustup target add wasm32-wasip2`.
