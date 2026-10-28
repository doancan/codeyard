## What and why

<!-- One or two sentences. -->

Closes #<!-- issue number: links this pull request to the issue, moves it to In Review, closes it on merge -->

## Checklist

- [ ] `scripts/check.sh` passes locally (fmt, clippy, arch-check, docs-check, cargo deny, audit, tests; see [docs/testing.md](https://github.com/doancan/codeyard/blob/main/docs/testing.md))

CI runs these gates. `scripts/memcheck.sh` also runs in CI on macOS, but its budgets are only authoritative on a Retina Mac;
if the change could add memory or binary size, say so above and run `scripts/check.sh --budgets` on a Mac if you can.

House rules ([CONTRIBUTING.md](https://github.com/doancan/codeyard/blob/main/CONTRIBUTING.md)):

- [ ] Comments, docs and messages are in English
- [ ] Tests with the change (a bug fix starts with a failing test; failure paths are tested too)
- [ ] Docs updated, line added under *Unreleased* in `CHANGELOG.md`
- [ ] `UPDATE_DOCS=1 cargo test -p app docs_reference` run if settings changed
- [ ] No new dependency, or the reason is stated above
- [ ] GPUI patches untouched, or `vendor/gpui/PATCHES.md` and `scripts/check-gpui-patches.sh` updated

Interface text and translations ([docs/translating.md](https://github.com/doancan/codeyard/blob/main/docs/translating.md)), when the change adds or edits UI text or a language:

- [ ] New text goes through `t` / `tf` / `tn` / `tr` and has an entry in every Complete language; `locales/template.tsv` refreshed
- [ ] Translation changes: reviewed by someone who speaks the language natively (say who), seen running in the app, coverage from `scripts/i18n-status.sh` stated
