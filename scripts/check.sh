#!/usr/bin/env bash
# Run the gates CI runs before opening a PR (--budgets adds the memory budgets).
#   scripts/check.sh              everything below
#   scripts/check.sh --quick      skip cargo test
#   scripts/check.sh --strict     a missing cargo-deny / cargo-audit is a failure, not a skip
#   scripts/check.sh --budgets    also run scripts/memcheck.sh (macOS only; authoritative on a Retina display)
# Order: fmt, clippy, arch-check, docs-check, licences, cargo deny, audit, test.
set -euo pipefail
cd "$(dirname "$0")/.."

quick=0 strict=0 budgets=0
for a in "$@"; do
  case "$a" in
    --quick) quick=1 ;;
    --strict) strict=1 ;;
    --budgets) budgets=1 ;;
    -h|--help) sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "check: unknown option '$a' (try --help)" >&2; exit 2 ;;
  esac
done

step() { printf '\n== %s\n' "$*"; }

# optional <tool> <install hint> <command...>
optional() {
  local tool=$1 hint=$2; shift 2
  if command -v "$tool" >/dev/null; then
    "$@"
  elif [ "$strict" = 1 ]; then
    echo "check: $tool is not installed ($hint) and --strict is set" >&2
    exit 1
  else
    echo "skipped: $tool is not installed ($hint)"
  fi
}

step "cargo fmt"
cargo fmt --all --check

step "cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "scripts/arch-check.sh"
scripts/arch-check.sh

step "scripts/docs-check.sh"
scripts/docs-check.sh

step "scripts/licenses.sh"
scripts/licenses.sh
python3 -I scripts/test-crate-licenses.py
python3 -I scripts/crate-licenses.py --check

step "project board rules"
python3 -I scripts/test-project-sync.py

step "cargo deny"
optional cargo-deny "cargo install cargo-deny --locked" cargo deny check

step "scripts/audit.sh"
optional cargo-audit "cargo install cargo-audit --locked" scripts/audit.sh

if [ "$quick" = 0 ]; then
  step "cargo test"
  cargo test --workspace
fi

if [ "$budgets" = 1 ]; then
  if [ "$(uname -s)" = Darwin ]; then
    step "scripts/memcheck.sh"
    scripts/memcheck.sh
  else
    echo "skipped: scripts/memcheck.sh needs macOS"
  fi
fi

printf '\ncheck: ok\n'
