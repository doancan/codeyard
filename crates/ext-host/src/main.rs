//! `codeyard-exthost`: runs one extension's WebAssembly component for the app, over stdin and stdout
//! (docs/plugin-architecture.md §3). The only binary that links wasmtime and Cranelift; the app never does
//! (`scripts/arch-check.sh`).
//!
//!   codeyard-exthost --compile < extension.wasm > compiled.cwasm
//!   codeyard-exthost                (the app speaks `ext-proto` on stdin/stdout)
#![deny(clippy::unwrap_used)]

mod bindings;
mod convert;
mod engine;
mod prints;
mod run;
mod state;
mod storage;

use std::io::{Read, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("--compile") => compile(),
        None => run::run(std::io::stdin().lock(), std::io::stdout()),
        Some(other) => Err(format!("unknown argument {other}; usage: codeyard-exthost [--compile]")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("codeyard-exthost: {e}");
            ExitCode::FAILURE
        }
    }
}

fn compile() -> Result<(), String> {
    let mut wasm = Vec::new();
    std::io::stdin().read_to_end(&mut wasm).map_err(|e| e.to_string())?;
    let engine = engine::engine().map_err(|e| e.to_string())?;
    let code = engine::compile(&engine, &wasm).map_err(|e| format!("{e:#}"))?;
    std::io::stdout().write_all(&code).map_err(|e| e.to_string())
}
