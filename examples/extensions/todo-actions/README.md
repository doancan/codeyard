# TODO Actions

Code actions for TODO comments in Rust and Markdown: one edits the document (mark it done), the other runs a command of the extension (copy the line into a list kept in storage). Like formatters, code actions get their document from Codeyard and need no permission.

Each TODO in view also gets a code lens, "Collect", which runs the extension's `collect` command with the TODO as its argument, and an inlay hint after it with its place among the file's TODOs (`2 of 5`). Codeyard asks for lenses and hints for the lines in view only, again after an edit and after a scroll.

```sh
cargo test     # unit tests, against a mock Codeyard
cyx test       # the built extension, in the real extension host
cyx dev        # run it in Codeyard; saving a file rebuilds and reloads it
```

You need Rust and its WebAssembly target: `rustup target add wasm32-wasip2`.
