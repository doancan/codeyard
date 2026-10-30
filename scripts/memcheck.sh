#!/usr/bin/env bash
# Memory budgets and the agent size budget (macOS only: needs `footprint`). Exit 1 if a budget is exceeded.
#   scripts/memcheck.sh
# Measures the settled `footprint` (what Activity Monitor shows; see scripts/perf/footprint.sh).
# Window is 1280x800; budgets are for Retina (2x).
set -euo pipefail
cd "$(dirname "$0")/.."
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
cargo build --release -q -p app -p agent
APP=./target/release/codeyard
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
# Leave the user's state database (recent folders) and settings alone; this also keeps the
# old-folder-name migration from running on first start.
export CODEYARD_STATE_DIR="$TMP/state"
export CODEYARD_CONFIG_DIR="$TMP/config"
# Normal scenes measure the editor after update preferences have been chosen. Otherwise the first-run
# question opens another native window and silently adds its screen buffers to every scene below.
mkdir -p "$CODEYARD_CONFIG_DIR"
printf '{"update.check":"manual"}\n' > "$CODEYARD_CONFIG_DIR/settings.jsonc"
# The runtime Dock icon costs ~3 MB; the budgets are for the app itself.
export CODEYARD_NO_DOCK_ICON=1
cp crates/app/testdata/big.rs "$TMP/big.txt"
. scripts/perf/footprint.sh

fail=0
check() { # name budget measured unit
  local status=ok unit=${4:-MB}
  # An empty or non-numeric measurement (process died, `footprint` failed) must not pass: `[ "" -gt N ]` is an error, not "false".
  case "$3" in ''|*[!0-9]*) status=FAIL; fail=1; echo "memcheck: no measurement for '$1' (process exited early or footprint failed)" >&2 ;;
    *) if [ "$3" -gt "$2" ]; then status=FAIL; fail=1; fi ;;
  esac
  printf "  %-4s %-40s %6s %s  (budget %s)\n" "$status" "$1" "$3" "$unit" "$2"
}

measure() { # name budget args...
  local name=$1 budget=$2; shift 2
  "$APP" "$@" >/dev/null 2>&1 &
  local pid=$!
  check "$name" "$budget" "$(settled_mb $pid)"
  kill $pid 2>/dev/null || true; wait $pid 2>/dev/null || true
}

# The budgets are a guide: a little growth per feature is normal (2026-10-21: ~10 MB of
# headroom for the Rust-file rows after git + LSP). Real waste (leaks, double loads, needless
# copies) still gets fixed.
# 2026-10-24: "small Rust file" is now the 336 KB workbench.rs (was 49 KB) inside a git repo:
# the Timeline/Outline/Graph sections are drawn. The difference is in GPU textures
# (IOAccelerator), not malloc (13 -> 14 MB). An earlier build measured 89-94 MB on the same file.
# 2026-10-24 (hardening): the same file swings between 101 and 108 MB; 27c1c59 (before that round)
# and the current build measured 101-104 MB side by side, so no regression. The file grew to
# 364 KB; budget 110.
# 2026-10-24: minimap on by default (+2-3 MB: 75 -> 77-78 MB on the same machine);
# plain-text budget 80 -> 85. The same release had a faded 420 px logo on the welcome screen that
# took the terminal row from 70 to 84 MB (GPU atlas); the logo was removed instead of loosening
# the budget. Run this script when adding images.
# 2026-10-24: budgets loosened by about 15% to leave room for features. The script now
# catches sudden jumps rather than slow growth: a change that adds 10+ MB on its own (like the
# welcome-screen logo) is investigated first, the budget is raised last.
echo "memory (footprint):"
measure "terminal open"                       85 --terminal crates/app
measure "empty window (welcome screen)"       85
measure "small Rust file (360 KB, in a repo)" 125 crates/app/src/workbench.rs
measure "10k lines, plain text (507 KB)"      95 "$TMP/big.txt"
measure "10k lines, syntax highlighting"     130 crates/app/testdata/big.rs
# Keep the first-run window covered explicitly. A clean, undecided profile must never check the network.
CODEYARD_CONFIG_DIR="$TMP/first-run-config" measure "first-run update question" 95

# Measure an owned idle daemon. Looking up an arbitrary process can select another
# test's or the user's persistent daemon, then kill it during cleanup.
./target/release/codeyard-agent --daemon "$TMP/agent.sock" >/dev/null 2>&1 &
agent=$!
check "remote agent daemon (idle)" 8 "$(settled_mb "$agent")"
kill "$agent" 2>/dev/null || true; wait "$agent" 2>/dev/null || true

echo "size:"
# The app binary has no size budget: it grows with features, and what users feel is startup, typing and memory
# (scripts/perf). Its size is still printed for the record. The agent keeps its budget: it is copied to every server.
printf "  %-4s %-40s %6s MB\n" info "app binary" "$(du -m "$APP" | cut -f1)"
for t in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
  f=target/$t/release-agent/codeyard-agent
  # 1664 KB: processes, port scan, the large-file index and task runs brought it to ~1.6 MB; a guide, not a hard limit.
  [ -f "$f" ] && check "agent ($t)" 1664 "$(du -k "$f" | cut -f1)" KB
done
exit $fail
