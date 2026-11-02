# Contributing

Thanks for helping. This file is the short path from a clone to a change that passes every gate.

## Setup

- Rust stable (edition 2024). macOS: Xcode with the Metal toolchain (`xcodebuild -downloadComponent MetalToolchain`).
- For SSH agents: `cargo install cargo-zigbuild`, `zig`, and `rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl`.
- Optional tools used by some tests: `git`, `sshd` (for the SSH integration tests), `python3`.

```
cargo run --release -p app -- path/to/folder
cargo test --workspace
```

Point runs at throw-away profile folders so you never touch your real settings:
`CODEYARD_STATE_DIR=/tmp/cy-state CODEYARD_CONFIG_DIR=/tmp/cy-config cargo run -p app`.

## Where things live

Read [docs/architecture.md](docs/architecture.md) first (crates, the workbench topic-file layout, size limits).
New workbench state goes into a group in `workbench_state.rs`; new behaviour goes into a `workbench_<topic>.rs` file;
only `crates/app` may depend on GPUI.

## Issues and labels

Every issue is opened from a form in `.github/ISSUE_TEMPLATE/`: **Epic** (a goal made of several issues), **Feature** (a
user-visible capability, written as a specification), **Task** (work with no user-visible behaviour of its own), **Bug**,
**Translation** and **Accessibility barrier** (`6-accessibility.yml`, see [ACCESSIBILITY.md](ACCESSIBILITY.md)). Questions
and open ideas go to Discussions.

- **Title:** a plain summary, at most 78 characters, with no type prefix (the type label says it).
  Features and tasks start with a verb ("Show …", "Pass …"); bugs say what goes wrong.
- **Labels:** two or three, at most one from each group. The form sets the type; triage adds the area, and the environment
  when the issue depends on it (leave it off when local or remote makes no difference, such as docs).

| Group | Labels |
| --- | --- |
| Type | `epic`, `feature`, `task`, `bug`, `accessibility` (a barrier for people with disabilities, triaged as a bug) |
| Area | `lsp`, `debug`, `editor`, `git`, `remote`, `terminal`, `ui`, `i18n`, `docs`, `infra` (build, release, test harness) |
| Environment | `local` (only local folders), `ssh` (only remote folders), `local+ssh` (both alike) |
- **Relationships** live in the issue's sidebar, never in its text: a feature or task is a sub-issue of its epic, and
  *blocked by* names the issues it waits for. An epic is closed when all its sub-issues are.
- **Milestone:** the release the issue ships in (`v0.7.0`). Its due date is the epic's Target date.

Every issue is on the [Codeyard project](https://github.com/users/doancan/projects/2). Priority, effort and dates are project
fields, set at triage:

| Field | Values |
| --- | --- |
| Status | `Backlog` → `In Progress` → `In Review` → `Test` → `Done`; `Blocked` from any open stage |
| Priority | `P0` broken for everyone, drop other work · `P1` next up · `P2` planned · `P3` when there is time |
| Effort | `XS` ≤ 2 hours · `S` < 1 day · `M` 1–3 days · `L` ≤ 1 week · `XL` more, split it before starting |
| Start date, Target date | Shown on the Roadmap view |

Every move is automatic: `.github/workflows/project.yml` runs `scripts/project-sync.py` on issue, pull request, review
and CI events, and once an hour to catch what sends no event (relationships edited in the sidebar). Nothing is moved by
hand; a status set by hand stays until an event gives a reason to move it.

| Stage | Entered when |
| --- | --- |
| Backlog | The issue is opened or reopened, unassigned, or its last blocker closes while nobody is assigned |
| In Progress | Someone is assigned (`scripts/issue-start.sh N` assigns you and checks out the linked branch), a draft pull request that says `Closes #N` is opened, or a review asks for changes. Start date is set the first time |
| In Review | The pull request is ready for review, or CI fails on it while it is in Test |
| Test | The review approves, or CI passes on the ready pull request: what is left is the issue's manual checks |
| Done | The issue is closed (merging a pull request that says `Closes #N` closes it) |
| Blocked | A *blocked by* issue is open; it leaves Blocked by itself when the last one closes |

Opening a pull request for an unassigned issue assigns it to the pull request's author. An epic follows its sub-issues:
In Progress once one has started, Test when all are closed (its own verification list is left), Done when it is closed.

## Before you open a pull request

Run one command:

```
scripts/check.sh
```

It runs fmt, clippy, `arch-check.sh`, `docs-check.sh`, `licenses.sh` (the licence texts that releases ship), `cargo deny check`, `scripts/audit.sh` and `cargo test --workspace`,
in that order, and stops at the first failure. `--quick` skips the tests. `cargo-deny` and `cargo-audit` are skipped with a
message when missing (`cargo install cargo-deny cargo-audit --locked`); `--strict` makes that a failure. Details of each
gate are in [docs/testing.md](docs/testing.md).

CI enforces everything above on pull requests: macOS tests and memory budgets, Windows native tests and
standalone packaging, Linux library/agent checks, and the full Linux workspace (`linux-app`) are blocking.
`scripts/memcheck.sh` runs in CI as a blocking step on macOS, but the budgets are set for a Retina display and the runner
has a 1x display and measures lower, so CI only catches gross regressions. The authoritative numbers come from a Retina
Mac (`scripts/check.sh --budgets`); if your change could add memory or binary size, run it there if you can and say so.
Tag releases use Developer ID signing and Apple notarization in CI; see [release evidence](docs/releasing.md).

House rules:

1. **Tests with the change.** A bug fix starts with a failing test. Failure paths and the messages users see are tested as
   carefully as the happy path. Tests are hermetic: no real home directory, no network, no stray processes.
2. **Docs with the change.** Update `docs/` for architecture or behaviour changes, add a line under *Unreleased* in
   `CHANGELOG.md`, and regenerate the reference with `UPDATE_DOCS=1 cargo test -p app docs_reference` when settings change.
   `scripts/docs-check.sh` fails when the documents and the code disagree.
3. **Budgets are real.** A change that adds 10 MB or more is investigated before any budget is raised.
4. **No new dependency without a reason.** `cargo deny` checks licences, advisories and sources and `scripts/audit.sh` (RustSec) must stay green; an accepted advisory is an explicit, justified entry in `.cargo/audit.toml`; keep `Cargo.lock` committed.
5. **Small commits** with a message that says why.
6. **English in the repository.** Comments, documentation, commit messages, script output and test names are English. Only the
   translation tables in `crates/app/locales/` and deliberate example text in tests are in other languages.
7. **New UI strings.** Text users read goes through `crate::i18n` (`t`, `tf`, `tn` for counts, `tr` for text drawn later than it is
   created) and gets an entry in every **Complete** language (`crates/app/locales/tr.tsv`); never build a plural from an English
   `s` and never join translated fragments. The tests in `i18n_tests.rs` fail when a call site, a palette label or a menu item
   has no entry, when an entry matches no English string, and when `locales/template.tsv` is stale
   (`UPDATE_I18N=1 cargo test -p app --bin codeyard i18n_template` refreshes it). Command labels (`Command::action("…")`) and
   `run_command_for_test` stay English; the palette translates at display time. `i18n_lint_tests` fails when a literal is given straight to a
   text sink (`.child("…")`, `Notice::…`, tooltips, prompts); a legitimate exception carries `// i18n-keep: reason`. The full rules, the file format and how to add a
   language are in [docs/translating.md](docs/translating.md); `scripts/i18n-status.sh` shows coverage.
8. **Translations are reviewed like code.** Say who reviewed the text and whether they speak the language natively; machine
   translation is welcome as a first draft, not as a submission.

## Licence of contributions

Codeyard is licensed `MIT OR Apache-2.0`. By submitting a contribution you agree that it is licensed under both, without additional terms.

## Releases

Maintainers only; see [docs/releasing.md](docs/releasing.md).
