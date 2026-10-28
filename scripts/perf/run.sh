#!/usr/bin/env bash
# Measure startup, typing latency and memory of a Codeyard build, optionally side by side with a baseline (macOS only).
#   scripts/perf/run.sh                          the working tree, written to target/perf/candidate.json
#   scripts/perf/run.sh --baseline v0.5.0        also build v0.5.0 and run both builds interleaved
#   scripts/perf/run.sh --aa                     A/A: the working tree against itself, to see the noise
#   scripts/perf/run.sh --runs 20 --mem-runs 5   runs per timing scenario / per memory scene (defaults 20 / 5)
# Then: scripts/perf/compare.py target/perf/baseline.json target/perf/candidate.json
# A baseline without the CODEYARD_PERF hooks (crates/app/src/perf.rs) gives memory rows only.
set -euo pipefail
cd "$(dirname "$0")/../.."
[ "$(uname)" = Darwin ] || { echo "perf: macOS only (needs footprint)" >&2; exit 2; }
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

runs=20 mem_runs=5 baseline="" aa=0
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
case "$runs$mem_runs" in *[!0-9]*) echo "perf: --runs and --mem-runs take a number" >&2; exit 2 ;; esac
[ "$runs" -ge 1 ] && [ "$mem_runs" -ge 1 ] || { echo "perf: --runs and --mem-runs must be at least 1" >&2; exit 2; }

OUT=target/perf
mkdir -p "$OUT"
TMP=$(mktemp -d)
pid=""
# Stop a running app too: an interrupted run must not leave Codeyard open on a deleted profile.
trap '[ -n "$pid" ] && kill "$pid" 2>/dev/null; rm -rf "$TMP"' EXIT
trap 'exit 130' INT TERM
. scripts/perf/footprint.sh

cargo build --release -q -p app -p cyx
cp target/release/codeyard "$OUT/codeyard-candidate"
names=(candidate)
commits=("$(git rev-parse HEAD)")
versions=("$(git describe --tags --always --dirty)")
if [ -n "$baseline" ]; then
  rev=$(git rev-parse --verify "$baseline^{commit}")
  wt="$OUT/worktree-$rev"
  # Kept between runs so the baseline is built once; `prune` forgets one whose folder was deleted (cargo clean).
  git worktree prune
  [ -d "$wt" ] || git worktree add -q --detach "$wt" "$rev"
  # Its own target directory: sharing one would rebuild both revisions on every run.
  (cd "$wt" && CARGO_TARGET_DIR="$PWD/../target-$rev" cargo build --release -q -p app)
  cp "$OUT/target-$rev/release/codeyard" "$OUT/codeyard-baseline"
  names+=(baseline)
  commits+=("$rev")
  versions+=("$(git describe --tags --always "$rev")")
elif [ "$aa" = 1 ]; then
  cp "$OUT/codeyard-candidate" "$OUT/codeyard-baseline"
  names+=(baseline)
  commits+=("${commits[0]}")
  versions+=("${versions[0]}")
fi

# A fresh profile per start: no recent folders, no restored tabs, no first-run question, no Dock icon. With PROFILE_FROM
# set, a copy of that prepared profile instead.
fresh_profile() {
  rm -rf "$TMP/state" "$TMP/config"
  if [ -n "${PROFILE_FROM:-}" ]; then
    cp -R "$PROFILE_FROM/state" "$TMP/state"
    cp -R "$PROFILE_FROM/config" "$TMP/config"
    return
  fi
  mkdir -p "$TMP/config"
  printf '{"update.check":"manual"}\n' > "$TMP/config/settings.jsonc"
}

# A profile with 100 installed extensions (themes made by `cyx new`), built once per run with the candidate.
EXT_PROFILE="$TMP/ext-profile"
mkdir -p "$EXT_PROFILE/config" "$EXT_PROFILE/state" "$TMP/gen"
printf '{"update.check":"manual"}\n' > "$EXT_PROFILE/config/settings.jsonc"
CYX="$PWD/target/release/cyx"
for n in $(seq 1 100); do
  (cd "$TMP/gen" && "$CYX" new theme "t$n" --publisher perf >/dev/null)
  "$CYX" package "$TMP/gen/t$n" -o "$TMP/gen/t$n.cyx" >/dev/null
done
install_args=()
for f in "$TMP"/gen/*.cyx; do install_args+=(--install-extension "$f"); done
CODEYARD_CONFIG_DIR="$EXT_PROFILE/config" CODEYARD_STATE_DIR="$EXT_PROFILE/state" "$OUT/codeyard-candidate" "${install_args[@]}" >/dev/null
installed=$(grep -c '"digest"' "$EXT_PROFILE/config/extensions.lock.jsonc" 2>/dev/null || echo 0)
[ "$installed" = 100 ] || { echo "perf: the extension profile has $installed extensions, not 100" >&2; exit 1; }
launch() { # app args... (background; $! is the pid)
  local app=$1; shift
  CODEYARD_STATE_DIR="$TMP/state" CODEYARD_CONFIG_DIR="$TMP/config" CODEYARD_NO_DOCK_ICON=1 "$app" "$@" >/dev/null 2>&1 &
}
# A scripted run quits by itself (CODEYARD_PERF_EXIT); a build without the hooks never does, so stop it after 30 s.
run_scripted() { # app args...
  fresh_profile
  launch "$@"
  pid=$!
  local i
  for i in $(seq 1 300); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  pid=""
}
count() { grep -c "\"$1\"" "$2" || true; } # metric file

for n in "${names[@]}"; do : > "$TMP/$n.jsonl"; done

# Timing scenarios, interleaved so that thermal state and background load hit every build alike; the order alternates
# so that neither build always runs first. A build without the hooks (a release before them) is timed once only.
export CODEYARD_PERF_EXIT=1
untimed=" "
for i in $(seq 1 "$runs"); do
  order=("${!names[@]}")
  [ $((i % 2)) = 0 ] && [ ${#order[@]} = 2 ] && order=(1 0)
  for k in "${order[@]}"; do
    case "$untimed" in *" $k "*) continue ;; esac
    export CODEYARD_PERF="$TMP/${names[$k]}.jsonl"
    # The welcome screen with nothing installed: its startup marks, not the (empty) extension load.
    : > "$TMP/welcome.jsonl"
    CODEYARD_PERF="$TMP/welcome.jsonl" run_scripted "$OUT/codeyard-${names[$k]}"
    grep -v '"startup.extensions_ms"' "$TMP/welcome.jsonl" >> "$CODEYARD_PERF" || true
    if [ "$(count startup.first_frame_ms "$TMP/welcome.jsonl")" != 1 ]; then
      if [ "$i" = 1 ] && [ "${names[$k]}" = baseline ]; then
        echo "perf: the baseline has no CODEYARD_PERF hooks; it is compared on memory only" >&2
        untimed="$untimed$k "
        continue
      fi
      echo "perf: no first frame from ${names[$k]}: the window must be visible (display awake, not covered, same Space)" >&2
      exit 1
    fi
    cp crates/app/testdata/big.rs "$TMP/big.rs"
    # Only its typing samples count: its startup opens a file, a different scenario from the welcome screen above.
    : > "$TMP/typing.jsonl"
    CODEYARD_PERF="$TMP/typing.jsonl" CODEYARD_PERF_TYPING=40 run_scripted "$OUT/codeyard-${names[$k]}" "$TMP/big.rs"
    # The scenario saves after typing: if the keys did not reach the editor, the samples measure nothing.
    if [ "$(count typing.keystroke_frame_ms "$TMP/typing.jsonl")" != 40 ] || ! grep -q "^a\{40\}" "$TMP/big.rs"; then
      echo "perf: the typing run of ${names[$k]} did not type and draw 40 keys; the window must be visible and nothing else may type into it" >&2
      exit 1
    fi
    grep '"typing\.' "$TMP/typing.jsonl" >> "$CODEYARD_PERF" || true
    # 100 installed extensions: only the time to load them counts.
    : > "$TMP/ext.jsonl"
    CODEYARD_PERF="$TMP/ext.jsonl" PROFILE_FROM="$EXT_PROFILE" run_scripted "$OUT/codeyard-${names[$k]}"
    grep '"startup.extensions_ms"' "$TMP/ext.jsonl" >> "$CODEYARD_PERF" || true
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
      pid=""
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
  python3 -I scripts/perf/collect.py "$TMP/${names[$k]}.jsonl" "$c" "${versions[$k]}" > "$OUT/${names[$k]}.json"
  echo "perf: wrote $OUT/${names[$k]}.json"
done
