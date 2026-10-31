#!/usr/bin/env bash
# Rebuild codeyard-exthost's test extension: crates/ext-host/tests/guests/probe → crates/ext-host/testdata/probe.wasm
# (and probe-escape.wasm, with the sandbox escape attempts, probe-guards.wasm, for the app's run-time guards, and
# probe-scm.wasm, a toy source control provider),
# and the performance scenarios' extensions (scripts/perf/run.sh): the decoration provider
# crates/ext-host/tests/guests/decorator → crates/ext-host/testdata/decorator.wasm, the 10 000-row view
# crates/ext-host/tests/guests/viewlist → crates/ext-host/testdata/viewlist.wasm, and the two builds of the hot-reload
# subject crates/ext-host/tests/guests/reload → crates/ext-host/testdata/reload-a.wasm and reload-b.wasm.
#   scripts/build-exthost-fixtures.sh
# Needs the wasm32-wasip2 Rust target (rustup target add wasm32-wasip2). The .wasm is committed, so tests need no
# WebAssembly toolchain; run this after changing the guest or the WIT.
# testdata/probe-0.1.wasm is not rebuilt: it is the probe as built against API 0.1.0, kept to show that the exthost
# still runs 0.x components during the move to 1.0 (tests/exthost.rs, a_component_built_against_0_1_still_runs).
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
# The same guest with a toy source control provider (the app's Source Control tests).
(cd crates/ext-host/tests/guests/probe && CARGO_TARGET_DIR="$target" cargo build --release --target wasm32-wasip2 --quiet --features scm)
cp "$target/wasm32-wasip2/release/probe.wasm" crates/ext-host/testdata/probe-scm.wasm
(cd crates/ext-host/tests/guests/decorator && CARGO_TARGET_DIR="$target" cargo build --release --target wasm32-wasip2 --quiet)
cp "$target/wasm32-wasip2/release/decorator.wasm" crates/ext-host/testdata/decorator.wasm
(cd crates/ext-host/tests/guests/viewlist && CARGO_TARGET_DIR="$target" cargo build --release --target wasm32-wasip2 --quiet)
cp "$target/wasm32-wasip2/release/viewlist.wasm" crates/ext-host/testdata/viewlist.wasm
# Two builds that differ in one constant, swapped in place by the hot-reload scenario.
(cd crates/ext-host/tests/guests/reload && CARGO_TARGET_DIR="$target" cargo build --release --target wasm32-wasip2 --quiet)
cp "$target/wasm32-wasip2/release/reload.wasm" crates/ext-host/testdata/reload-a.wasm
(cd crates/ext-host/tests/guests/reload && CARGO_TARGET_DIR="$target" cargo build --release --target wasm32-wasip2 --quiet --features b)
cp "$target/wasm32-wasip2/release/reload.wasm" crates/ext-host/testdata/reload-b.wasm
ls -l crates/ext-host/testdata/*.wasm
