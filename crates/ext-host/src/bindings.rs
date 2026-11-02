//! wasmtime's bindings for the `extension` world of `codeyard:api` (crates/ext-api/wit).

wasmtime::component::bindgen!({ path: "../ext-api/wit", world: "extension" });
