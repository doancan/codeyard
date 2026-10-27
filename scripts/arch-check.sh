#!/usr/bin/env bash
# Architecture budget: files and the `Workbench` struct must not grow (limits only go down).
#   scripts/arch-check.sh
# Exit 1 when a limit is exceeded. For new state, add a field group in `workbench_state.rs`
# (see docs/architecture.md); split the file instead of raising the limit.
set -euo pipefail
cd "$(dirname "$0")/.."
fail=0
check() { # file limit
  local n; n=$(wc -l < "$1")
  if [ "$n" -gt "$2" ]; then echo "FAIL $1: $n lines (limit $2)"; fail=1; else echo "ok   $1: $n / $2"; fi
}
# Known large files: limits sit just above today's size; lower them as files get split.
check crates/app/src/workbench.rs 2600
check crates/app/src/editor.rs 1700
check crates/buffer/src/lib.rs 1800
# Every other source file (vendor and generated code excluded). New files that are not yet `git add`ed count too.
files=$(git ls-files --cached --others --exclude-standard 'crates/*.rs') || { echo "arch-check: git ls-files failed (not a git checkout?)" >&2; exit 2; }
files=$(printf '%s\n' "$files" | grep -v -e '/grammars/' -e '/testdata/' || true)
[ -n "$files" ] || { echo "arch-check: no crates/*.rs files found" >&2; exit 2; }
while IFS= read -r f; do
  [ -f "$f" ] || continue # in the index but deleted from the working tree
  case "$f" in
    crates/app/src/workbench.rs|crates/app/src/editor.rs|crates/buffer/src/lib.rs) continue ;; # checked above
    */tests.rs|*/qa_*_tests.rs) check "$f" 6000 ;;
    *) check "$f" 2500 ;;
  esac
done <<< "$files"
# Length of the `Workbench` struct (one line per field plus doc lines).
n=$(awk '/^pub struct Workbench( |<|\{)/{f=1} f{c++} f&&/^}/{print c; exit}' crates/app/src/workbench.rs)
case "$n" in ''|*[!0-9]*) echo "FAIL Workbench struct: 'pub struct Workbench {' ... '}' not found in crates/app/src/workbench.rs"; fail=1 ;; *)
  if [ "$n" -gt 80 ]; then echo "FAIL Workbench struct $n lines (limit 80): put the new field in a state group"; fail=1; else echo "ok   Workbench struct: $n / 80"; fi ;;
esac
# vendor/ patches still in place (vendor/gpui/PATCHES.md).
scripts/check-gpui-patches.sh || fail=1
exit $fail
