#!/usr/bin/env bash
# Prints the id of the prebuild run (.github/workflows/prebuild.yml, a run on develop) that built the same sources as HEAD, or
# nothing when there is none. A run that is still going is waited for (up to --wait-minutes, default 45): its files are
# nearly always finished sooner than a new build would be. "The same sources" is the same git tree, whatever the commit: the
# merge of develop into main has the tree of develop's last commit.
#   scripts/find-prebuilt.sh [--wait-minutes N]
# Exit status 3: a build of these sources exists but its full CI run failed (the release must stop).
# Needs `gh` signed in (GH_TOKEN) and the recent history of develop (`git fetch --depth 100 origin develop`).
set -euo pipefail
cd "$(dirname "$0")/.."

wait_minutes=45
if [ "${1:-}" = --wait-minutes ]; then wait_minutes=${2:?--wait-minutes needs a number}; fi
tree=$(git rev-parse 'HEAD^{tree}')
deadline=$(( $(date +%s) + wait_minutes * 60 ))

while :; do
  pending=0
  runs=$(gh run list --workflow prebuild.yml --branch develop --limit 40 --json databaseId,headSha,status,conclusion \
    -q '.[] | [.databaseId, .headSha, .status, .conclusion] | @tsv')
  while IFS=$'\t' read -r id sha status conclusion; do
    [ -n "${id:-}" ] || continue
    git cat-file -e "$sha^{commit}" 2>/dev/null || continue
    [ "$(git rev-parse "$sha^{tree}")" = "$tree" ] || continue
    if [ "$status" = completed ]; then
      if [ "$conclusion" = success ]; then
        # The full test run (ci.yml) of the same commit must not have failed. One that is still going is not waited for: a
        # release should not sit behind the Windows tests; a failure of it is the signal not to release these sources.
        if gh run list --workflow ci.yml --branch develop --commit "$sha" --json conclusion -q '.[].conclusion' | grep -qx failure; then
          echo "the full CI run of develop failed for the commit $sha (same sources): fix it and push again" >&2
          exit 3
        fi
        echo "$id"
        exit 0
      fi
    else
      pending=1
    fi
  done <<< "$runs"
  if [ "$pending" = 1 ] && [ "$(date +%s)" -lt "$deadline" ]; then sleep 30; else break; fi
done
