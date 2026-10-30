# TODO Actions

Code actions for TODO comments in Rust and Markdown: one edits the document (mark it done), the other runs a command of the extension (copy the line into a list kept in storage). Resting the mouse on `TODO`, `FIXME` or `NOTE` explains it next to the language server's hover, and in a `//` comment the markers join the completion list. Like formatters, code actions, hover and completion get their document from Codeyard and need no permission.

```sh
cargo test     # unit tests, against a mock Codeyard
cyx test       # the built extension, in the real extension host
cyx dev        # run it in Codeyard; saving a file rebuilds and reloads it
```

You need Rust and its WebAssembly target: `rustup target add wasm32-wasip2`.
