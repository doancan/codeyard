//! Rust toolchain integration beyond the language server's basics: runnables, rust-analyzer's health and flycheck,
//! the server controls, rustc error explanations, cross-compilation targets and cargo-nextest.
//! The files here are plain logic (no GPUI); the workbench side is `workbench_rust.rs` and its sub-topics.

#[path = "rust_explain.rs"]
pub mod explain;
#[path = "rust_hints.rs"]
pub mod hints;
#[path = "rust_nextest.rs"]
pub mod nextest;
#[path = "rust_probe.rs"]
pub mod probe;
#[path = "rust_runnables.rs"]
pub mod runnables;
#[path = "rust_status.rs"]
pub mod status;
#[path = "rust_target.rs"]
pub mod target;
#[path = "rust_views.rs"]
pub mod views;
