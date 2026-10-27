#!/usr/bin/env bash
# Start work on an issue: assign it to you and create its linked branch (the issue's Development section), checked out.
#   scripts/issue-start.sh 17
# The project workflow (.github/workflows/project.yml) then moves it to In Progress and sets its Start date.
set -euo pipefail
cd "$(dirname "$0")/.."
[ $# -eq 1 ] && [[ $1 =~ ^[0-9]+$ ]] || { echo "usage: scripts/issue-start.sh ISSUE_NUMBER" >&2; exit 2; }
n=$1
repo=doancan/codeyard

state=$(gh issue view "$n" --repo "$repo" --json state --jq .state)
[ "$state" = OPEN ] || { echo "issue-start: #$n is $state" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "issue-start: commit or stash your changes first" >&2; exit 1; }

gh issue edit "$n" --repo "$repo" --add-assignee @me >/dev/null
# Reuse the linked branch if there is one, otherwise create it from main.
branch=$(gh issue develop "$n" --repo "$repo" --list 2>/dev/null | awk 'NR==1 {print $1}')
if [ -n "$branch" ]; then
  git fetch -q origin "$branch" && git checkout -q "$branch"
else
  gh issue develop "$n" --repo "$repo" --base main --checkout >/dev/null
  branch=$(git branch --show-current)
fi
echo "#$n: assigned to you, on branch $branch"
