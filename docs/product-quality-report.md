# Product quality and Rust hardening delivery

Date: 2026-10-28. Review platform: macOS on Apple silicon, native GPUI application.

The delivery combines the existing main history, all develop work through `2ea3ad8`, and the product/Rust fixes in
`143c747`. Integration commit `f5dcdb2` preserves both branches' accessibility work and the container refinements.
The delivery pull request is the authoritative record of the final tree's cross-platform CI checks and merge.

## Delivered behavior

- Rust automatic debugging discovers actual Cargo workspace members, binaries, examples, required features and
  library-only test harnesses. Builds use literal argv; executable paths come from Cargo, including custom output directories.
- Debugging saves open files in the active root before discovery/build/start, preserving format-on-save and conflict
  protection. Save failures, deleted files, Stop and workspace changes prevent an outdated or unwanted launch.
- Stop invalidates pending discovery/test-build replies. An ended session rejects late attachment, startup failure and
  stopped events. A Cargo command already running may finish in the background without reviving the cancelled session.
- Rust test commands use the nearest actual manifest and crate-relative test ID. Parameterized async attributes and raw
  identifiers are discovered; Debug Test at Cursor refuses unrelated helper functions.
- Failed or interrupted test builds reject partial executables and publish readable compiler errors in Output/Problems.
  Workspace test runs continue through target failures; duplicate names and missing results cannot leave misleading green results.
- Rust commands distinguish disabled features, workspace trust and server readiness. Opening Cargo.toml works without
  a language server and stays inside the file's workspace. Delayed replies cannot operate on a different workspace.
- Source Control exposes its branch, message, commit state, change groups, diffs, staging and results through AccessKit.
  Actions reuse real Git operations and reject stale state. Hidden Explorer trees are omitted, stale row actions resolve
  paths again, and Right Arrow stays on an expanded empty folder.
- The Run and Debug selector explains automatic project detection and identifies active automatic sessions.
  Existing web preview, container overview/detail/log improvements and landmark accessibility geometry are retained.
- Memory checks measure and stop their own disposable agent daemon. The shared save coordinator avoids three copies
  of the same asynchronous state machine in the release binary. Unused translation keys were removed after integration.

## Validation evidence

| Check | Result and tree covered |
| --- | --- |
| `scripts/check.sh --strict` with required SSH tests, timeout scale 5 and two test threads | Passed before the final cancellation/integration changes: 1,698 workspace tests passed, five documented ignores; 1,347 application tests passed |
| `scripts/check.sh --quick --strict` | Passed after integration: formatting, Clippy, architecture, docs, licenses, project rules, dependency policy and RustSec audit |
| `cargo test --locked -p app --bin codeyard rust_hardening` | 19 passed after integration, including real Cargo builds, actual workspace/features/output paths, save conflicts, Stop and Cargo→DAP launch |
| `cargo test --locked -p app --bin codeyard product_` | Eight passed after integration, including real Git stage/unstage/commit through AccessKit requests |
| `cargo test --locked -p app --bin codeyard a11y` | 21 passed after integration, including layout/scale/visibility/split-editor geometry and AccessKit consumer validation |
| `cargo test --locked -p app --bin codeyard container` | 35 passed after integration; one documented live-engine acceptance test ignored |
| `cargo test --locked -p app --bin codeyard i18n_` | 50 passed after the integration's two dead translation keys and stale template were corrected |
| `scripts/memcheck.sh` on Retina | Final run passed all seven memory scenes and all three size checks; existing budgets unchanged |

The initial baseline had ten debugger/SSH timeouts and was not treated as green. The subsequent complete local run
passed with the environment above; both real LLDB failures also passed independently. The initial product regressions,
initial Rust regressions, stale-green-result regression and both late-cancellation regressions failed before their fixes.

The final release binary measured 62,606,560 bytes, within the existing 60 MiB limit. The idle owned agent measured
2 MB; Linux agent artifacts measured 1,568/1,360 KiB, within their existing limits. The final Retina window readings
were 78 MB (terminal), 74 MB (empty), 98 MB (small Rust file), 89 MB (10k plain-text lines), 128 MB (10k highlighted
lines) and 82 MB (first-run question), all within their existing budgets.

Earlier Retina runs captured transient readings above budget, including 305 MB at first launch. An independent
90-second first-launch measurement stayed at 73–74 MB, and the complete final rerun passed. Those earlier failures
are retained in the local validation logs; a single passing sample is not evidence of every long-session scenario.

## Native walkthrough

A separate Codeyard QA bundle used disposable settings/state and a generated Rust/Git repository with a temporary
local bare remote. The user's running editor and real repositories were not used for destructive acceptance steps.

- Quick Open, Rust text/tab exposure and keyboard navigation worked in the native window.
- Right Arrow on the expanded empty Explorer folder stayed on the folder.
- Source Control's native accessibility tree exposed Stage/Unstage. Both actions changed the disposable index as expected;
  switching to Source Control omitted the hidden Explorer from the tree.
- Rust: Open Cargo.toml opened the correct manifest without an attached language server.
- A real LLDB session stopped inside the Rust program built into `custom-build`; a separate Debug Test at Cursor session
  reached the test's assertion in its Cargo-produced test executable.
- The final native confirmation showed the new Auto-detect project label. The owned QA application was closed afterwards.

## Remaining product work

The scoped implementation and integration are complete. The [development plan](product-quality-plan.md) retains the
remaining product backlog with acceptance criteria: control/text-level accessibility geometry and silent panels,
VoiceOver/Windows assistive-technology testing on hardware, interface scaling, Explorer multi-selection, large-repository
Git performance, and unified Rust features/profiles/presets. Native Windows hardware, every Rust macro/test framework,
frame timings and active-accessibility CPU cost are not certified by this macOS walkthrough.

New Turkish strings were drafted by the coding agent; no human native-speaker review is claimed.
