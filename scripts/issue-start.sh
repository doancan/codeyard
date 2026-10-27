#!/usr/bin/env bash
# Start work on an issue: assign it to you, create its linked branch (the issue's Development section) and check it
# out, and move it to In Progress on the project board with today's Start date (kept if one is already set).
#   scripts/issue-start.sh 17
# Needs `gh` with the `project` scope (`gh auth refresh -s project`). The other status changes are the project's
# built-in workflows; see "Issues and the project board" in CONTRIBUTING.md.
set -euo pipefail
cd "$(dirname "$0")/.."
[ $# -eq 1 ] && [[ $1 =~ ^[0-9]+$ ]] || { echo "usage: scripts/issue-start.sh ISSUE_NUMBER" >&2; exit 2; }
n=$1
owner=doancan
project=2
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

url=$(gh issue view "$n" --repo "$repo" --json url --jq .url)
item=$(gh project item-add "$project" --owner "$owner" --url "$url" --format json --jq .id)
project_id=$(gh project view "$project" --owner "$owner" --format json --jq .id)
fields=$(gh project field-list "$project" --owner "$owner" --format json)
status_field=$(jq -r '.fields[] | select(.name == "Status") | .id' <<<"$fields")
in_progress=$(jq -r '.fields[] | select(.name == "Status") | .options[] | select(.name == "In Progress") | .id' <<<"$fields")
start_field=$(jq -r '.fields[] | select(.name == "Start date") | .id' <<<"$fields")

gh project item-edit --id "$item" --project-id "$project_id" --field-id "$status_field" --single-select-option-id "$in_progress" >/dev/null
start=$(gh project item-list "$project" --owner "$owner" --format json --limit 500 --jq ".items[] | select(.id == \"$item\") | .[\"start date\"] // empty")
[ -n "$start" ] || gh project item-edit --id "$item" --project-id "$project_id" --field-id "$start_field" --date "$(date +%F)" >/dev/null

echo "#$n: assigned to you, In Progress, on branch $branch"
