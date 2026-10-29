#!/usr/bin/env bash
# Rebuild codeyard-exthost's test extension: crates/ext-host/tests/guests/probe → crates/ext-host/testdata/probe.wasm.
#   scripts/build-exthost-fixtures.sh
# Needs the wasm32-wasip2 Rust target (rustup target add wasm32-wasip2). The .wasm is committed, so tests need no
# WebAssembly toolchain; run this after changing the guest or the WIT.
set -euo pipefail
cd "$(dirname "$0")/.."
target=$(mktemp -d "${TMPDIR:-/tmp}/exthost-fixtures.XXXXXX")
trap 'rm -rf "$target"' EXIT
(cd crates/ext-host/tests/guests/probe && CARGO_TARGET_DIR="$target" cargo build --release --target wasm32-wasip2 --quiet)
cp "$target/wasm32-wasip2/release/probe.wasm" crates/ext-host/testdata/probe.wasm
ls -l crates/ext-host/testdata/probe.wasm
