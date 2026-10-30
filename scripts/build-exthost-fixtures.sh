#!/usr/bin/env bash
# Rebuild codeyard-exthost's test extension: crates/ext-host/tests/guests/probe → crates/ext-host/testdata/probe.wasm
# (and probe-escape.wasm, with the sandbox escape attempts, and probe-guards.wasm, for the app's run-time guards).
#   scripts/build-exthost-fixtures.sh
# Needs the wasm32-wasip2 Rust target (rustup target add wasm32-wasip2). The .wasm is committed, so tests need no
# WebAssembly toolchain; run this after changing the guest or the WIT.
set -euo pipefail
cd "$(dirname "$0")/.."
target=$(mktemp -d "${TMPDIR:-/tmp}/exthost-fixtures.XXXXXX")
trap 'rm -rf "$target"' EXIT
(cd crates/ext-host/tests/guests/probe && CARGO_TARGET_DIR="$target" cargo build --release --target wasm32-wasip2 --quiet)
cp "$target/wasm32-wasip2/release/probe.wasm" crates/ext-host/testdata/probe.wasm
# The same guest with the `escape` command, whose socket imports a package could never carry (the import check).
(cd crates/ext-host/tests/guests/probe && CARGO_TARGET_DIR="$target" cargo build --release --target wasm32-wasip2 --quiet --features escape)
cp "$target/wasm32-wasip2/release/probe.wasm" crates/ext-host/testdata/probe-escape.wasm
# The same guest with the commands of the app's run-time guards (clipboard, run, hold).
(cd crates/ext-host/tests/guests/probe && CARGO_TARGET_DIR="$target" cargo build --release --target wasm32-wasip2 --quiet --features guards)
cp "$target/wasm32-wasip2/release/probe.wasm" crates/ext-host/testdata/probe-guards.wasm
ls -l crates/ext-host/testdata/probe.wasm crates/ext-host/testdata/probe-escape.wasm crates/ext-host/testdata/probe-guards.wasm
