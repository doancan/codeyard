# Translating Codeyard

The interface language is chosen with `"ui.language"` (`auto`, `en`, or a language code; see the
[user guide](user-guide.md#interface-language)). This document is for people who add or improve a translation, and for
developers who add UI text. Translations are plain text files compiled into the binary: no tools, no extra dependency, no
network, and a missing entry is never an error at run time (that one string simply stays English).

## Languages

| Code | Language | Status |
| --- | --- | --- |
| `en` | English | Source |
| `tr` | Türkçe | Complete |

**Status** decides what the tests demand:

- **Source**: English, the language of the keys.
- **Complete**: every string the program can show through the translation system has an entry. The tests enforce this, so a change
  that adds UI text must update every Complete language in the same change. New languages start as Partial.
- **Partial**: the file may miss entries (they show in English). The tests still check the file itself: format, placeholders,
  plural forms, no dead keys. Say in the pull request who reviewed it. A Partial language can become Complete when it covers every
  string (see `scripts/i18n-status.sh`) and someone commits to keeping it current.

## How it works

The key is the English text exactly as written in the source: `t("Delete branch {}?")`. Nothing else identifies a string, so
**changing an English sentence orphans its translations** (the dead-key test then names the old key). Rename the key in every
`<code>.tsv` in the same change.

| In code | Use |
| --- | --- |
| `t("Save")` | a fixed text |
| `tf("Delete branch {}?", &[&name])` | text with values (`{}` next value, `{0}`, `{1}` by position) |
| `tn("{} file", "{} files", n, &[])` | a count: the first value is `n`, further values follow |
| `tr("Search…")` | text drawn later than it is created (a field's placeholder): marks the key, `t()` runs when drawn |

Command labels (`Command::action("…")`) stay English in the source; the palette translates them when it draws and still finds a command by
its English label. Command ids, key bindings, setting keys and the values in `settings.jsonc` are never translated.

### Plurals

Do not build plurals from English suffixes (`format!("{n} file{}", if n == 1 { "" } else { "s" })`): no other language can translate that.
Use `tn(singular, plural, n, args)`. In the `.tsv` the key joins both English forms with `|` and the translation lists **the forms of your
language** separated by `|`; the number of forms comes from `plural` in `LANGUAGES`:

| `Plural::` | forms | languages (CLDR) |
| --- | --- | --- |
| `None` | 1 | Turkish, Japanese, Chinese, Korean, Vietnamese, Indonesian |
| `OneOther` | 2: one, other | English, German, Spanish, Italian, Dutch, Swedish, Portuguese (Portugal) |
| `French` | 2: 0 and 1, other | French, Portuguese (Brazil) |
| `EastSlavic` | 3: 1 / 2–4 / 0, 5+ | Russian, Ukrainian, Belarusian |
| `Polish` | 3: 1 / 2–4 / 0, 5+ | Polish |

```
{} file|{} files	{} dosya
{} файл|{} файла|{} файлов
```

A language whose rule is not in the table (Arabic, Czech, Welsh…) needs a new `Plural` variant with CLDR-based tests before it
can be added; open an issue first.

### What is not supported (yet)

- **Right-to-left** languages (Arabic, Hebrew, Persian): the editor and layout are left-to-right only.
- **Context for identical English words** (the verb *Open* and the noun *Open*): one English text has one translation. If a word needs two
  translations, change the English text in the source to tell them apart (for example `Open File…`).
- **Number, date and currency formats** follow no locale; day and month names are ordinary entries (`Monday`, `Oct`).
- **Documentation** (user guide, README) is English. Only the program interface is translated.
- **Regional variants** work as a fallback chain: `pt-BR` uses a `pt-BR` entry if it exists, otherwise `pt`. Register a variant only when
  the text really differs.

## Add a language

1. `scripts/i18n-new-language.sh pt-BR "Português (Brasil)"` copies `crates/app/locales/template.tsv` to `locales/pt-BR.tsv` and prints
   the lines to add in step 3.
2. Translate the right-hand column. A line left empty means "not translated yet" and is skipped. The script also starts
   `locales/help/<code>.tsv`, the explanations behind the **?** of each setting: copy the lines of `help/en.tsv` you translate
   (a setting without a line shows the English text; see [settings-help.md](settings-help.md)).
3. Register it in `crates/app/src/i18n.rs`: one `Language { … }` in `LANGUAGES` (code, native name, `include_str!` of both files, plural rule,
   `status: Status::Partial`) and the code in `SETTING_CHOICES`, and add a row to the table above.
4. `UPDATE_DOCS=1 cargo test -p app docs_reference` (the `ui.language` values are in `docs/settings.md`), then
   `cargo test -p app --bin codeyard i18n`.
5. `scripts/i18n-status.sh` shows the coverage; `I18N_LIST=1 scripts/i18n-status.sh` lists the missing strings.
6. Look at it running: `CODEYARD_STATE_DIR=/tmp/s CODEYARD_CONFIG_DIR=/tmp/c codeyard --panel welcome` with `"ui.language": "pt-BR"`
   in `/tmp/c/settings.jsonc`. Menus, palette, Welcome and the panels you translated must read naturally and fit (long German or Finnish
   words overflow buttons first).
7. Open the pull request (the template has a translation checklist). Add a line to `CHANGELOG.md`.

### File format

One entry per line, `English<TAB>Translation`. Empty lines and lines starting with `#` are comments. No tabs inside, no leading or trailing
spaces, one line per entry. Rules the tests check for every language:

- no duplicate keys, no keys that are not real English strings (typos, old text);
- a translation uses only placeholders its key has (it may leave one out when the grammar allows) and a key with placeholders is
  translated with placeholders;
- a plural entry has exactly the number of forms of the language's rule, none empty;
- a label that ends in `…` (it opens a dialog) ends in `…` in the translation too.

## Quality bar

A translation is reviewed like code. It must:

- read as written by a native speaker, not as a word-by-word rendering. Prefer the term users already know from the operating
  system and from other editors in that language; do not invent words.
- use **one term for one concept** throughout. Start a glossary comment at the top of the file (see `tr.tsv`) and follow it.
- keep the register of the English: short imperative for buttons and menus, full sentences with a period for messages, the same
  capitalisation convention the language uses for menus.
- keep product and tool names untouched (Codeyard, Git, SSH, LSP, JSON, GitHub) and keep Git commands as people type them (commit, stash,
  pull, push, fetch, rebase) unless the language has a firmly established term.
- not translate file names, paths, shortcuts, setting keys, commands someone would type, or placeholders.
- fit: a menu or button label that is much longer than the English probably needs rephrasing.

Machine translation is fine as a first draft; it is not acceptable to submit it unreviewed. Say in the pull request who reviewed the
text and whether they speak the language natively.

### Turkish (`tr`) style

The rules the current file follows. The text was written with these rules but has **not yet had a native-speaker review**; corrections
from Turkish speakers are the most useful contribution right now.


- Address the user formally and politely (*siz*): "Önce bunları çözün", never "çöz".
- Buttons and menu items use the bare imperative stem (*Kaydet*, *Aç*, *Klonla*); nouns for items that name a thing (*Yeni Dosya*,
  *Zaman Çizelgesi*). Title Case like the English menus.
- Messages are sentences with a final period, like the English.
- Turkish has no grammatical plural after a number (`{} dosya`), so plural entries have one form.
- The glossary is the comment block at the top of `crates/app/locales/tr.tsv`: it is the single source of truth, so a new term goes
  there in the same change. Examples: Source Control *Kaynak Denetimi*, Explorer *Gezgin*, Workspace *Çalışma Alanı*, Branch *Dal*,
  Problems *Sorunlar*, Debug *Hata Ayıklama*, Breakpoint *Kesme Noktası*, Host *Sunucu*, Merge *Birleştir*.
- Git terms stay as they are (Git, commit, stash, pull, push, fetch, rebase) and take a Turkish auxiliary verb where a verb is needed
  (*Commit Et*, *Dalı Rebase Et*). A Git term identical to its English key is left out of `t()` (the tests treat an identical
  entry as untranslated).
- Apostrophe for suffixes after proper names and numbers (*codeyard'dan Çık*).

## For developers: adding UI text

1. Put text that users read through `t`, `tf`, `tn` or `tr` (above). A string literal passed straight to `.child("…")`, `Notice::Info("…")`,
   a tooltip, a placeholder, `confirm(…)` or `show_input(…)` fails `i18n_lint_tests`. Text that must not be translated (a product or
   protocol name, a host name, someone else's output) is marked on its line with `// i18n-keep: reason`, so the exception is visible in
   review. The scan is a heuristic over single-line calls: it catches what it names, it does not prove that nothing was missed.
2. Add the entry to `crates/app/locales/tr.tsv` (Turkish is Complete, so the tests fail without it) and run
   `UPDATE_I18N=1 cargo test -p app --bin codeyard i18n_template` to refresh `template.tsv` (a test fails when it is stale).
3. Never concatenate translated fragments: a sentence is one key with placeholders, because word order differs between languages.
4. Do not put markup, punctuation that belongs to the sentence, or leading/trailing spaces outside the key.
5. New plural? `tn`, never an `s` suffix.
6. A Turkish entry may not equal its English key (the tests treat that as untranslated); a Git term that stays as it is, is left out of `t()`.

## Pull request checklist (translations)

- [ ] `cargo test -p app --bin codeyard i18n` passes
- [ ] `scripts/i18n-status.sh` coverage stated in the description
- [ ] reviewed by someone who speaks the language natively (name them or say "me")
- [ ] looked at the running app in this language (a screenshot helps)
- [ ] `docs/translating.md` table and `CHANGELOG.md` updated
- [ ] you agree that the text is licensed `MIT OR Apache-2.0` like the rest (see [CONTRIBUTING.md](../CONTRIBUTING.md))
