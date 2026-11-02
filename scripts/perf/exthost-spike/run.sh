#!/usr/bin/env bash
# Phase 2 spike (#97): build the test extensions and the throwaway exthost, then print the measurements.
#   scripts/perf/exthost-spike/run.sh [out-dir]
# Needs the wasm32-wasip2 Rust target and Node.js. On macOS the exthost is signed ad hoc with the hardened runtime
# and exthost.entitlements, loads code with SPIKE_LOAD=copy-text and runs under Seatbelt: the shipping setup.
set -euo pipefail
cd "$(dirname "$0")"
here=$(pwd)
out=${1:-$(mktemp -d "${TMPDIR:-/tmp}/exthost-spike.XXXXXX")}
mkdir -p "$out"
out=$(cd "$out" && pwd)
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$out/target}
exe_suffix=""
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) exe_suffix=".exe" ;; esac

(cd guest-rust && cargo build --release --target wasm32-wasip2 --quiet)
(cd guest-rust-big && cargo build --release --target wasm32-wasip2 --quiet)
(cd host && cargo build --release --quiet)
wasm=$CARGO_TARGET_DIR/wasm32-wasip2/release
(
  cd guest-js
  npm ci --no-audit --no-fund --silent
  node build.mjs "$out/js-hello.wasm" hello.js
  # An edited build: what saving a source file in `cyx dev` produces.
  sed "s/hello /hello again /" hello.js > hello.b.js
  node build.mjs "$out/js-hello.b.wasm" hello.b.js
  rm -f hello.b.js
  npx esbuild octokit.js --bundle --format=esm --platform=neutral --main-fields=module,main \
    --external:codeyard:spike/host@0.1.0 --log-level=warning --outfile=octokit.bundle.js
  node build.mjs "$out/js-octokit.wasm" octokit.bundle.js
  rm -f octokit.bundle.js
)

exe=$CARGO_TARGET_DIR/release/spike-exthost$exe_suffix
if [ "$(uname -s)" = Darwin ]; then
  cp "$exe" "$out/spike-exthost"
  exe=$out/spike-exthost
  codesign -f -s - --options runtime --entitlements "$here/exthost.entitlements" "$exe" 2>/dev/null
  export SPIKE_LOAD=copy-text SPIKE_SANDBOX=1
fi

echo "$(uname -sm), $(git rev-parse --short HEAD), wasmtime $(grep -A1 'name = "wasmtime"' host/Cargo.lock | sed -n 's/version = "\(.*\)"/\1/p' | head -1)"
echo
"$CARGO_TARGET_DIR/release/spike-measure$exe_suffix" "$exe" "$out/measure" \
  rust-hello="$wasm/spike_guest.wasm" js-hello="$out/js-hello.wasm" js-hello.b="$out/js-hello.b.wasm" js-octokit="$out/js-octokit.wasm" \
  rust-1.6mb="$wasm/spike_guest_big.wasm"
