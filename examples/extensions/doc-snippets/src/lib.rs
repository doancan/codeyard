//! The Rust snippets of docs/extensions, each in the code it needs to compile (README.md). The snippets are written for
//! reading, so they keep names they do not use and closures that ignore their arguments.
#![allow(dead_code, unused_variables, unused_imports, unused_mut, non_upper_case_globals, clippy::all)]

// Formatting would reflow the snippets so they no longer match the docs.
#[rustfmt::skip]
mod commands;
#[rustfmt::skip]
mod developer_guide;
#[rustfmt::skip]
mod testing;
#[rustfmt::skip]
mod views;
