# Doc snippets

The Rust snippets of [docs/extensions](../../../docs/extensions), each inside the code it needs to compile: one file per
page, one module per snippet, named after the line it starts on. CI builds and tests this crate like the other
examples, and `cargo test -p cyx doc_tests` (crates/cyx/src/doc_tests.rs) checks that every `rust` snippet in the docs
appears in an example line by line. When you change a snippet, change it here too.

It is not an extension: there is no manifest, and nothing here is meant to be installed.
