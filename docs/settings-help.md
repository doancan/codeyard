# Settings help

Every row of the Settings page has a **?** button. It opens, under the description, an explanation in the interface language and the
facts of the setting: its key in `settings.jsonc`, the values it accepts and its default. Search looks in these texts too.

## Where the text lives

The explanations are data, not code. One file per language in `crates/app/locales/help/`:

| File | Language |
| --- | --- |
| `en.tsv` | English, the source (a setting that has no line in the current language shows this one) |
| `tr.tsv` | Türkçe |

A line is `setting key`, a TAB and the text; `\n` is a line break; a line starting with `#` is a comment. Example:

```
lsp.enabled	Turns language servers on or off. …\nWith this off the editor still highlights syntax.
```

A setting without a line still has its one-line description (`CATALOG` in `workbench_settings.rs`) and the facts, which the page
works out from the setting itself (`settings_help.rs`), so the **?** works for every setting.

## Adding or changing help

1. Add the line to `help/en.tsv`; the key must be a setting of the page (`CATALOG`).
2. Add the translation to every other file. Keep the words in backticks (values, keys) identical to the English: a test compares them.
3. Run `cargo test -p app settings_help`.

No Rust code changes. A **new language** adds `help/<code>.tsv` and one `help: include_str!(…)` in its `i18n::LANGUAGES` entry
(see [translating.md](translating.md)); the text of the facts (`Default: …`) is translated in the language's own `<code>.tsv`.

## Categories

The page groups settings by what they are about; the category of a key is decided in `settings_page::category_of`.
"Language & Region" holds the languages people write in (the interface language, spelling); "Programming Languages" holds language
servers, JSON/YAML schemas, `.http`, Emmet and the Rust and Cargo build settings; "Remote & Security" holds port forwarding, remote
Local History and Workspace Trust. A test checks that every setting of the page lands in a category.
