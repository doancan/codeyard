#!/usr/bin/env bash
# The "a command in under 5 minutes" goal (docs/plugin-architecture.md §9), scripted: `cyx new command`, its unit
# tests, then `cyx test` (build for wasm32-wasip2, run in codeyard-exthost), each step timed. It starts in an empty
# folder with its own cargo target folder, so the SDK is compiled from scratch as for a new author; only the toolchain
# and the crates cargo already downloaded are reused. Fails when the steps take more than 300 s together. With Node.js
# 22 or newer the TypeScript template runs the same way (`npm install`, `npm test`, `cyx test`).
#   scripts/check-five-minutes.sh
# Needs: Rust with the wasm32-wasip2 target. CI runs it in the Linux job.
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$PWD
budget=300

cargo build --locked --quiet -p cyx -p ext-host
cyx=$repo/target/debug/cyx
export CODEYARD_EXTHOST=$repo/target/debug/codeyard-exthost
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

now() { python3 -I -c 'import time; print(time.time())'; }
failed=0

# run_steps NAME DIR STEP… : each STEP is "label|command", run in DIR (the first in $work).
run_steps() {
  local name=$1 dir=$2
  shift 2
  local start total step label cmd t0 t1
  start=$(now)
  echo "== $name"
  for step in "$@"; do
    label=${step%%|*}
    cmd=${step#*|}
    t0=$(now)
    if ! (cd "$dir" 2>/dev/null || cd "$work"; eval "$cmd") >"$work/out.txt" 2>&1; then
      cat "$work/out.txt"
      echo "FAILED: $label"
      failed=1
      return
    fi
    t1=$(now)
    printf '  %-34s %6.1f s\n' "$label" "$(python3 -I -c "print($t1 - $t0)")"
  done
  total=$(python3 -I -c "print(round($(now) - $start, 1))")
  if python3 -I -c "import sys; sys.exit(0 if $total <= $budget else 1)"; then
    echo "  total $total s (budget $budget s)"
  else
    echo "  total $total s: over the $budget s budget"
    failed=1
  fi
}

export CARGO_TARGET_DIR=$work/target
run_steps "Rust command" "$work/quick" \
  "cyx new command quick|cd '$work' && '$cyx' new command quick --publisher you --sdk '$repo'" \
  "cargo test (mock host)|cargo test --quiet" \
  "cyx test (wasm32-wasip2, exthost)|'$cyx' test"

if python3 -I -c 'import shutil, subprocess, sys; n = shutil.which("node"); sys.exit(0 if n and int(subprocess.check_output([n, "--version"], text=True).strip().lstrip("v").split(".")[0]) >= 22 else 1)'; then
  run_steps "TypeScript command" "$work/quick-ts" \
    "cyx new command quick-ts --lang ts|cd '$work' && '$cyx' new command quick-ts --lang ts --publisher you --sdk '$repo'" \
    "npm install|npm install --no-audit --no-fund" \
    "npm test (mock host)|npm test" \
    "cyx test (componentize, exthost)|'$cyx' test"
else
  echo "== TypeScript command: skipped (Node.js 22 or newer is not on PATH)"
fi

exit $failed
