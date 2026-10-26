# Testing

`cargo test --workspace` runs everything: about 620 tests in `app`, plus the other crates (host, schema, spell,
agent, proto, buffer, syntax, state, theme). The whole suite takes roughly 75 seconds on an Apple-silicon laptop.

## Layers

- **Unit tests** next to the code (`#[cfg(test)] mod tests`): pure logic such as the buffer, parsers, schema walker,
  SSH config editing, transfers, `.http` parsing, tokenizers.
- **Workbench tests** (`crates/app/src/tests.rs`, `qa_*_tests.rs`, `features_tests.rs`, `hardening_tests.rs`,
  `error_ui_tests.rs`, `ssh_error_tests.rs`): `gpui::TestAppContext` drives the real workbench, commands, key
  strokes and overlays without drawing frames. State is read through `*_for_test` accessors only.
  `qa_*` files hold end-to-end scenarios; `features_tests.rs` has one or more tests per larger feature
  (follow, SSH management, transfers, schemas, spell, merge, pull requests, `.http`, key login).
- **Integration tests with real processes.** `test_sshd.rs` starts a real `sshd` on a free port with throw-away keys
  (a mutex serialises users of it); `crates/app/tests/ssh_askpass.rs` runs the whole askpass chain against it.
  Git tests use real repositories (including a local bare "remote" for pull requests); `.http` tests use a tiny TCP
  server inside the test.
- **Fuzz and hostile-input tests** (`schema/src/fuzz_tests.rs`, colour round-trips, spell tokenizer, Emmet,
  EditorConfig, semantic tokens): random and adversarial input must produce a bounded, panic-free result.
- **Random sessions**: `CODEYARD_QA_SEEDS=N cargo test … qa_random_new_commands_many_seeds` types random keys and
  commands, then undoes everything and checks the original text, then redoes and checks the edited text.

## Rules

1. **Hermetic.** Tests never touch the real home directory, config, state or cache and never use the network.
   Use `setup_with_config(cx, &fresh(...))` or `CODEYARD_CONFIG_DIR` / `CODEYARD_STATE_DIR`; services that download
   (schemas, dictionaries) are disabled under `cfg!(test)`; LSP is turned off unless the test is about LSP.
2. **No real external tools by accident.** A test that would start `clangd`, `gh` or a browser sets the feature off or
   uses a fake `Runner`.
3. **Failure paths are tested as hard as success paths.** Every message that tells the user what to do has a test
   asserting the text (`explain_checkout`, `explain_curl`, SSH key setup, transfer failures).
4. **Fault injection.** Transfers are tested with unreadable sources, dropped connections, cancel at every stage,
   type conflicts, odd file names and symlinks; config editors with symlinks, CRLF, read-only and missing files.
5. **Bug first.** A bug found in use or review gets a failing test before the fix. Flaky tests are root-caused:
   the upload cleanup race was found that way (a half-copied folder survived when `tar` failed mid-stream).
6. **Time.** Tests wait on conditions (`wait_ticking`, `wait_secs`), never on fixed sleeps, and have an upper bound.

## Documentation is tested

- `scripts/docs-check.sh` compares documents with files: memory and size budgets (`docs/architecture.md` against
  `scripts/memcheck.sh` and CI), the protocol version and daemon timings (`docs/remote.md`), the size limits of
  `scripts/arch-check.sh`, every `CODEYARD_*` variable (`docs/environment.md`), relative links, the document index,
  the changelog (a section for the current version and for every git tag, link references) and the CLI flags in the README.
- `cargo test -p app --bin codeyard i18n` checks the translation tables against the code: every palette label and every `t("…")`
  call has an entry (palette coverage must be 100 %), no entry is dead, placeholders match, `auto` resolves from an injected locale
  (the tests never read the machine's language; `Settings::default().ui_language` is `en` under `cfg(test)`, the language is per test thread).
- `cargo test -p app docs_` checks `docs/settings.md` against the settings catalog (it is generated:
  `UPDATE_DOCS=1 cargo test -p app docs_reference`), that every setting the parser reads is documented, that the Keys table
  of the user guide only lists shortcuts that are really bound (macOS), and that the built-in theme list and counts match.

## Gates

On Windows, `scripts/check-windows.ps1` also validates agent inputs and checks that the release EXE contains both Linux
agents after copying it alone to an empty directory. The check also requires the Windows GUI subsystem and native
application icon resource, preventing extra console windows and generic EXE icons. `scripts/build-windows.ps1` performs that standalone check for
local builds too. Build-script validation tests can be run with
`rustc --edition=2024 --test crates/app/build.rs -o target/agent-build-tests.exe`, then `target/agent-build-tests.exe`.

`scripts/check.sh` runs all of these in order (`--quick` skips `cargo test`, `--strict` turns a missing `cargo-deny` or
`cargo-audit` into a failure instead of a skip, `--budgets` adds `scripts/memcheck.sh` on macOS). Run them before every
release (see `releasing.md`) and before a pull request:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
scripts/audit.sh           # cargo audit --deny warnings; exceptions with reasons in .cargo/audit.toml
scripts/arch-check.sh        # also runs scripts/check-gpui-patches.sh (vendor patches still present)
scripts/docs-check.sh
scripts/licenses.sh        # the licence files that releases ship exist; also checks THIRD_PARTY_CRATES.md (scripts/crate-licenses.py --check)
cargo test --workspace
scripts/memcheck.sh        # rerun once if the first Rust-file row spikes (GPU texture noise ±5–8 MB)
```

CI runs fmt, clippy, arch-check, docs-check, tests, `cargo deny` and the audit. `scripts/memcheck.sh` also runs on the macOS CI
runner as a blocking step, but that runner has a 1x display and measures lower, so it only catches gross regressions; the
authoritative numbers come from a Retina Mac. GitHub tag releases sign and notarize using Apple credentials in Actions
Secrets. The signed dry run and six mocked notarization/credential-cleanup tests are described in `releasing.md`.

`scripts/memcheck.sh` starts the release app in a scripted window, lets it settle and measures `footprint`; see
`architecture.md` for the budgets.

## CI build scheduling and caches

CI uses separate Rust dependency caches for macOS tests, Windows checks, Linux library tests, the Linux
app and coverage. macOS memory/size checks use the production release dependency cache (`app-macos`) and the same
macOS deployment target as the release bundle. Workspace crates are rebuilt normally; Cargo invalidates changed
dependencies, manifests, features, compiler versions and flags. Cache writes happen on main and explicit manual runs;
PR jobs restore the default branch cache without saving a separate copy. Coverage artifacts use a separate key.

The `macos-tests` and `macos-budgets` jobs run concurrently. The existing `macos` check waits for both and explicitly
fails for failure, cancellation or skipping of either job, so existing rules requiring `macos` still require the tests
and real memory/size measurement. Budget values, release profiles and test coverage are unchanged.

Windows native tests (`windows-tests`) and the actual release EXE (`windows-standalone`) also run in parallel. The existing
`windows` check requires both. The standalone job reuses the production `app-windows` cache and invokes
`build-windows.ps1 -Profile Release`, including GUI/icon/embedded-agent verification. Only CI calls
`check-windows.ps1 -SkipStandalone`; the default local command still runs every check.

`cargo-audit` and optional `cargo-llvm-cov` are installed from prebuilt tools rather than compiled on each runner.
RustSec advisories are still fetched and checked on every audit run. Metal's compiler is downloaded only if missing;
CI still fails before compilation if it cannot be invoked. GitHub checkout/artifact/cache actions run on Node.js 24.
A normal manual CI run does not start coverage; select the `coverage` input to request the instrumented report.
The Linux app job was promoted from experimental status after four consecutive successful full workspace test runs;
its errors now fail CI. The completed one-off Linux-to-Windows benchmark workflow is retired; its archived data remains
in `docs/benchmarks/windows-msvc-2026-10-26.json`.

### CI timings on 2026-10-26

The [previous main run](https://github.com/doancan/codeyard/actions/runs/40315596123) and the
[optimized manual run](https://github.com/doancan/codeyard/actions/runs/40322097282) used identical app/vendor source
and Cargo profiles. Both passed every executed check. The optimized run restored dependency caches; an earlier
[cache-seeding run](https://github.com/doancan/codeyard/actions/runs/40318390579) still had sequential Windows checks.

| Measurement | Previous CI | Optimized CI, cache hits |
| --- | ---: | ---: |
| Active workflow span, excluding initial queue | 27:14 | 13:34 |
| Windows test + standalone check family | 27:14 | 9:57 |
| macOS test + budget check family | 12:17 | 13:34 |
| Linux app workspace test job | 12:40 | 8:18 |
| RustSec audit job | 3:05 | 0:21 |

The active span was 50.2% shorter. The previous run also waited 12:26 before its first job, due to the existing
main-branch concurrency policy; that wait is excluded from the claimed improvement. The optimized run took 13:38
including startup. macOS did not get faster in these samples and became the final critical path. These are single
samples on different hosted machines; timings vary with runner hardware, load and cache availability. Native tests,
release optimization settings, budget thresholds and standalone checks were retained. Optional coverage was not run.

Raw run/job durations, source-tree hashes, cache-hit evidence and limits are in
[ci-2026-10-26.json](benchmarks/ci-2026-10-26.json). The final Linux policy was tightened after four actual workspace
test successes, not merely four green job summaries. The successful commands also cover that stricter policy; only
failure handling and the old label changed afterwards. New CI-specific keys must be populated by the first main run
before subsequent PRs can restore those main-branch caches.

## Coverage

`scripts/coverage.sh` (cargo-llvm-cov; `cargo install cargo-llvm-cov --locked`, `rustup component add llvm-tools`) runs the
whole workspace under instrumentation and writes `coverage/lcov.info` and `coverage/summary.json` (git-ignored) plus a per-crate
table. `vendor/`, test files (`tests.rs`, `*_tests.rs`, `tests/`, `test_*.rs`), `testdata/` and generated grammars are
excluded, so the figures describe production code only. `COVERAGE_CLEAN=1` deletes `target/llvm-cov-target` afterwards (it is
several GB). The build runs with `CARGO_PROFILE_DEV_DEBUG=0`, and the instrumented test run is slower than a plain one
(`app` tests: 75 s measured here, but 190 s in the first attempt on a loaded machine). CI has an optional manually started
`coverage` job (workflow_dispatch with the `coverage` checkbox enabled) that uploads `coverage/` as an artifact;
coverage is a measurement, not a gate.

Measured on 2026-10-25 (Apple silicon, macOS, line/function/region coverage of covered over instrumented):

| crate | lines | functions | regions |
| --- | --- | --- | --- |
| agent | 90.7% | 85.8% | 89.9% |
| app | 85.0% | 80.7% | 84.4% |
| buffer | 96.3% | 97.0% | 96.9% |
| host | 90.0% | 81.9% | 90.2% |
| proto | 86.9% | 87.5% | 82.7% |
| schema | 94.3% | 89.7% | 94.2% |
| spell | 91.7% | 85.8% | 91.3% |
| state | 97.2% | 98.8% | 92.5% |
| syntax | 96.2% | 93.2% | 96.7% |
| theme | 96.5% | 92.6% | 92.6% |
| **workspace** | **86.6%** | **82.3%** | **86.1%** |

Caveats, stated plainly:

- `app` is 80% of the code and the lowest crate: the GPUI rendering paths (`ui.rs`, element code) are only exercised by
  `TestAppContext`, which does not draw frames, and the platform glue (window, menus, accessibility, SSH askpass) needs a real window.
- The numbers exclude one test, `qa_debug_real_cargo_project_without_config`: it builds and debugs a real cargo project and needs
  debug info, which the coverage build switches off. It runs in `cargo test --workspace`.
- That run also had one failing test, `settings_docs::tests::docs_reference_settings_is_up_to_date`, because `docs/settings.md`
  was being edited at the time (regenerate with `UPDATE_DOCS=1 cargo test -p app docs_reference`). A failing test still contributes
  its coverage. `qa_terminal_ctrl_c_interrupts` timed out once in an earlier instrumented run and passed on the rerun (timing under load).
- Line coverage says a line ran, not that its result was checked.

## Driving the real window

A hidden flag runs palette commands in a real window after start: `codeyard --do "Command Name;Other Command"`
(debug builds only, or a release build with `CODEYARD_DEV=1` set; `CODEYARD_DO_DELAY_MS` sets the delay, `shortcut:<Command>` goes through the shortcuts page). `--panel` and the
screenshot helpers are used for visual checks; use throw-away `CODEYARD_STATE_DIR` / `CODEYARD_CONFIG_DIR`
directories. `scripts/a11y-probe/` reads the live accessibility tree.
