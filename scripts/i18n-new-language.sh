#!/usr/bin/env bash
# Start a new interface language: scripts/i18n-new-language.sh <code> "<native language name>"
#   scripts/i18n-new-language.sh pt-BR "Português (Brasil)"
# Copies `locales/template.tsv` to `locales/<code>.tsv`, starts `locales/help/<code>.tsv` (the "?" help of the settings) and prints
# the lines to add to i18n.rs.
# It does not register the language: someone who speaks it has to pick the plural rule
# (Plural::...), see docs/translating.md.
set -euo pipefail
cd "$(dirname "$0")/.."
code="${1:-}"; name="${2:-}"
if [ -z "$code" ] || [ -z "$name" ]; then
  echo "usage: $0 <code> \"<native language name>\"   e.g. $0 pt-BR \"Português (Brasil)\"" >&2
  exit 2
fi
if ! [[ "$code" =~ ^[a-z]{2,3}(-[A-Za-z0-9]{2,8})*$ ]]; then
  echo "error: '$code' is not a language tag like 'de', 'pt-BR' or 'zh-Hans'" >&2
  exit 2
fi
dest="crates/app/locales/$code.tsv"
if [ -e "$dest" ]; then echo "error: $dest already exists" >&2; exit 1; fi
{
  echo "# $name ($code) UI strings. Format: English<TAB>Translation. The left column is the English text exactly as in the source: do not edit it."
  echo "# An empty translation means \"not translated yet\" and is skipped. Plural entries (key with |): one form per plural form of the language, separated by |."
  echo "# Glossary: write the terms you chose here (Source Control, Workspace, Branch, …) and use them consistently. See docs/translating.md."
  echo
  grep -v '^#' crates/app/locales/template.tsv
} > "$dest"
echo "created $dest ($(grep -vc '^#\|^$' "$dest") strings to translate)"
help_dest="crates/app/locales/help/$code.tsv"
if [ ! -e "$help_dest" ]; then
  {
    echo "# $name ($code) settings help. Format: setting key<TAB>explanation; \\n is a line break. Same keys as help/en.tsv, keep the \`code\` words as they are."
    echo "# A setting without a line here shows the English help (docs/settings-help.md)."
  } > "$help_dest"
  echo "created $help_dest (translate the lines of help/en.tsv you want to offer)"
fi
cat <<NEXT

Next:
  1. Translate the right-hand column of $dest.
  2. In crates/app/src/i18n.rs add to LANGUAGES (choose the Plural:: rule of the language, see docs/translating.md):
       Language { code: "$code", name: "$name", tables: &[include_str!("../locales/$code.tsv")], help: include_str!("../locales/help/$code.tsv"), plural: Plural::OneOther,
                  case: crate::textcase::CaseLocale::Invariant, status: Status::Partial },
     and add "$code" to SETTING_CHOICES.
  3. Add a row to the table in docs/translating.md, run UPDATE_DOCS=1 cargo test -p app docs_reference,
     then cargo test -p app --bin codeyard i18n and scripts/i18n-status.sh.
NEXT
