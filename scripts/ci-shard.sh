#!/usr/bin/env bash
# One shard of the app crate's tests, for the pull request gate: `scripts/ci-shard.sh INDEX COUNT` (INDEX from 1).
# The tests are listed by the test binary, dealt out one by one (so every module is spread over all shards) and run by
# exact name; the shards run in parallel jobs that share one warm build cache. A test run on its own passes or fails the
# same as in the whole run: the few that share global state take `snippets::TEST_LOCK` or reset it themselves.
# CI sets CARGO_INCREMENTAL=1 and restores the incremental state of the last push to develop first.
set -euo pipefail
cd "$(dirname "$0")/.."

index=${1:?usage: ci-shard.sh INDEX COUNT} count=${2:?usage: ci-shard.sh INDEX COUNT}
if ! [ "$index" -ge 1 ] || ! [ "$index" -le "$count" ]; then
  echo "ci-shard: INDEX must be between 1 and COUNT" >&2
  exit 2
fi

exe=$(cargo test --locked -p app --bin codeyard --no-run --message-format=json | python3 -I -c '
import json, sys
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") == "compiler-artifact" and m.get("profile", {}).get("test") and m.get("target", {}).get("name") == "codeyard" and m.get("executable"):
        print(m["executable"])
')
[ -n "$exe" ] || { echo "ci-shard: the app test binary was not built" >&2; exit 1; }

names=$(mktemp)
trap 'rm -f "$names"' EXIT
"$exe" --list --format terse 2>/dev/null | sed -n 's/: test$//p' | sort | awk -v i="$index" -v n="$count" '(NR - 1) % n == i - 1' > "$names"
total=$(wc -l < "$names" | tr -d ' ')
echo "shard $index of $count: $total tests"
[ "$total" -gt 0 ] || { echo "ci-shard: no tests in this shard" >&2; exit 1; }
# Many names at once keep the process start-up out of the way; every call runs its tests in parallel itself.
xargs -n 600 "$exe" --exact --quiet < "$names"
