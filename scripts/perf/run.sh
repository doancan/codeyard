#!/usr/bin/env bash
# Measure startup, typing latency and memory of a Codeyard build, optionally side by side with a baseline (macOS only).
#   scripts/perf/run.sh                          the working tree, written to target/perf/candidate.json
#   scripts/perf/run.sh --baseline v0.5.0        also build v0.5.0 and run both builds interleaved
#   scripts/perf/run.sh --aa                     A/A: the working tree against itself, to see the noise
#   scripts/perf/run.sh --runs 20 --mem-runs 3   samples per timing scenario / per memory scene (defaults 20 / 3)
# Then: scripts/perf/compare.py target/perf/baseline.json target/perf/candidate.json
# A baseline without the CODEYARD_PERF hooks (crates/app/src/perf.rs) gives memory rows only.
set -euo pipefail
cd "$(dirname "$0")/../.."
[ "$(uname)" = Darwin ] || { echo "perf: macOS only (needs footprint)" >&2; exit 2; }
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

runs=20 mem_runs=3 baseline="" aa=0
while [ $# -gt 0 ]; do
  case "$1" in
    --baseline) baseline=$2; shift 2 ;;
    --runs) runs=$2; shift 2 ;;
    --mem-runs) mem_runs=$2; shift 2 ;;
    --aa) aa=1; shift ;;
    -h|--help) sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "perf: unknown option '$1' (try --help)" >&2; exit 2 ;;
  esac
done

OUT=target/perf
mkdir -p "$OUT"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
. scripts/perf/footprint.sh

cargo build --release -q -p app
cp target/release/codeyard "$OUT/codeyard-candidate"
names=(candidate)
commits=("$(git rev-parse HEAD)")
if [ -n "$baseline" ]; then
  rev=$(git rev-parse --verify "$baseline^{commit}")
  wt="$OUT/worktree-$rev"
  [ -d "$wt" ] || git worktree add -q --detach "$wt" "$rev"
  # Its own target directory: sharing one would rebuild both revisions on every run.
  (cd "$wt" && CARGO_TARGET_DIR="$PWD/../target-$rev" cargo build --release -q -p app)
  cp "$OUT/target-$rev/release/codeyard" "$OUT/codeyard-baseline"
  names+=(baseline)
  commits+=("$rev")
elif [ "$aa" = 1 ]; then
  cp "$OUT/codeyard-candidate" "$OUT/codeyard-baseline"
  names+=(baseline)
  commits+=("${commits[0]}")
fi

# A fresh profile per start: no recent folders, no restored tabs, no first-run question, no Dock icon.
fresh_profile() {
  rm -rf "$TMP/state" "$TMP/config"
  mkdir -p "$TMP/config"
  printf '{"update.check":"manual"}\n' > "$TMP/config/settings.jsonc"
}
launch() { # app args... (background; $! is the pid)
  local app=$1; shift
  CODEYARD_STATE_DIR="$TMP/state" CODEYARD_CONFIG_DIR="$TMP/config" CODEYARD_NO_DOCK_ICON=1 "$app" "$@" >/dev/null 2>&1 &
}
# A scripted run quits by itself (CODEYARD_PERF_EXIT); a build without the hooks never does, so stop it after 30 s.
run_scripted() { # app args...
  fresh_profile
  launch "$@"
  local pid=$! i
  for i in $(seq 1 300); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

for n in "${names[@]}"; do : > "$TMP/$n.jsonl"; done

# Timing scenarios, interleaved so that thermal state and background load hit every build alike.
export CODEYARD_PERF_EXIT=1
for i in $(seq 1 "$runs"); do
  for k in "${!names[@]}"; do
    export CODEYARD_PERF="$TMP/${names[$k]}.jsonl"
    run_scripted "$OUT/codeyard-${names[$k]}"
    cp crates/app/testdata/big.rs "$TMP/big.rs"
    CODEYARD_PERF_TYPING=40 run_scripted "$OUT/codeyard-${names[$k]}" "$TMP/big.rs"
    # The scenario saves after typing: if the keys did not reach the editor, the samples measure nothing.
    if grep -q '"typing.keystroke_frame_ms"' "$CODEYARD_PERF" && ! grep -q "^a\{40\}" "$TMP/big.rs"; then
      echo "perf: the typed keys did not reach the editor (${names[$k]}); typing samples are invalid" >&2
      exit 1
    fi
  done
  printf '\rperf: timing run %s/%s' "$i" "$runs" >&2
done
echo >&2
unset CODEYARD_PERF CODEYARD_PERF_EXIT

# Memory scenes: the method of scripts/memcheck.sh, a few samples each (a scene takes 20-50 s).
scenes=("memory.empty_window_mb|" "memory.syntax_10k_mb|crates/app/testdata/big.rs" "memory.terminal_mb|--terminal crates/app")
for i in $(seq 1 "$mem_runs"); do
  for k in "${!names[@]}"; do
    for s in "${scenes[@]}"; do
      metric=${s%%|*} args=${s#*|}
      fresh_profile
      # shellcheck disable=SC2086 # the scene's arguments are split on purpose
      launch "$OUT/codeyard-${names[$k]}" $args
      pid=$!
      v=$(settled_mb "$pid")
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
      case "$v" in ''|*[!0-9]*) echo "perf: no measurement for $metric (${names[$k]})" >&2 ;;
        *) printf '{"metric":"%s","value":%s}\n' "$metric" "$v" >> "$TMP/${names[$k]}.jsonl" ;;
      esac
    done
  done
  printf '\rperf: memory run %s/%s' "$i" "$mem_runs" >&2
done
echo >&2

for k in "${!names[@]}"; do
  c=${commits[$k]}
  python3 -I scripts/perf/collect.py "$TMP/${names[$k]}.jsonl" "$c" "$(git describe --tags --always "$c")" > "$OUT/${names[$k]}.json"
  echo "perf: wrote $OUT/${names[$k]}.json"
done
