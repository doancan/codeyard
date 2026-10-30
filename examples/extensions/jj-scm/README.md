# Jujutsu

A source control provider (`[[scm]]`) for [Jujutsu](https://jj-vcs.github.io/jj/) repositories. In a folder with a `.jj` folder, Source Control shows the working-copy change's files, the gutter compares open files with the parent change, the input box commits (`jj commit -m`), and each file's button restores it (`jj restore`). Git keeps every other folder.

Everything goes through `jj` with the `run` permission, so it works the same in a folder on an SSH host: the program runs there.

```sh
cargo test     # unit tests, against a mock Codeyard
cyx test       # the built extension, in the real extension host
cyx dev        # run it in Codeyard; saving a file rebuilds and reloads it
```

You need Rust and its WebAssembly target (`rustup target add wasm32-wasip2`), and `jj` on the workspace's host to use it.
