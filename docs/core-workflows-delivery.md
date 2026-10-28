# Core workflow delivery

Baseline: `85fbe76`, 2026-10-28. This is the acceptance ledger for eleven separately reviewed
feature pull requests. Git account sign-in, editor profiles/settings synchronization, and new
platform distribution work are outside this delivery. Existing supported platforms must keep
passing their checks. Do not mark a feature complete from unit tests alone.

## Shared definition of done

- The complete interaction is reachable from the interface and keyboard, follows existing
  semantic tokens, and has loading, empty, disabled, cancellation and actionable error states.
- Local and SSH workspaces use the Host abstraction. Root/workspace changes invalidate stale
  replies and pending confirmations. User data is never silently overwritten or discarded.
- Tests exercise actual side effects, partial failures, cancellation, Unicode/long paths,
  concurrent refreshes and restart behavior where applicable. Fixtures use disposable roots
  and profiles; no real server, account, home directory or running application is modified.
- New strings are localized; documentation, settings references and changelog match the code.
- Formatting, Clippy, architecture/documentation/localization/license/security checks and the
  workspace tests pass. Required macOS, Windows and Linux PR checks pass on the proposed head.
- A native-window acceptance pass verifies the changed interaction. Accessibility claims need
  real assistive-technology evidence; platform limitations and unperformed checks remain explicit.
- Memory/agent-size and performance gates pass. Large-data or rendering changes receive bounded workload measurements.
- Each feature has its own PR and professional commits describing the resulting behavior.
  PRs record validation and remaining limitations. Main receives only validated changes.

## Feature acceptance

| ID | Feature / proposed PR | Required behavior and hardening | Completion evidence |
| --- | --- | --- | --- |
| F01 | Select and manage multiple Explorer entries | Modifier-toggle and Shift-range mouse selection; keyboard range/select-all; stable path selection through refresh, sorting and collapse; selected-row semantics. Copy, cut, drag, paste and delete operate on the selection with one reviewable summary, parent/child deduplication, root protection, no overwrites, per-entry failures, retryable failed cuts, preserved dirty tabs and stale-host guards. File copying works through the agent without shell-specific commands. | Local strict checks, filesystem/GPUI/SSH coverage, native macOS copy walkthrough and Retina memory/size gates pass. PR #48 is ready for review; all required platform CI checks pass on `0293d1a`. The same-machine performance comparison remains pending. |
| F02 | Arrange editors in a persistent grid | Arbitrary nested horizontal/vertical groups; split in each direction; resize, move/reorder tabs, focus/navigation and close empty groups; shared document/independent views. Persist and validate layouts across restart, workspace changes, narrow windows and many groups; migrate existing two-group sessions. | Validated layout model, atomic session storage and GPUI workflows; 1,850 workspace tests and Retina memory budgets pass. Native restart/shared-edit/narrow-window flows are verified. Final native minimap confirmation, required CI on the latest fixes and performance comparison remain pending. |
| F03 | Set up language services from the editor | Discover bundled/installed JS/TS, HTML and CSS services; an explicit, discoverable install/retry flow with progress, cancellation, exact destination, offline errors and version/integrity controls. Install on the file's host; respect workspace trust and explicit user commands. Verify completion, diagnostics, formatting and restart without a terminal setup step. | Pending: fake installer/server failure matrix, local/SSH language interactions and native first-run flow. |
| F04 | Execute dependency-aware tasks | Typed shell/process tasks, cwd/env/platform options, variable/input expansion, parallel/sequence dependencies and cycle validation; background/watch readiness and custom problem matchers; pre/post-debug integration. Bounded output, cancellation of the process tree, concurrency policy, clear diagnostics and correct failed dependency behavior. | Pending: actual child-process DAG/watch/input/problem tests, local/SSH cancellation, debug integration. |
| F05 | Complete accessible application workflows | Search/results, Problems/fixes, Settings/shortcuts, debug/testing/remote views, SCM graph/worktrees/sync, output/ports, menus and dialogs expose roles, states, actions, errors and focus restoration. Row/control/text geometry, document-relative text selection, scroll actions and terminal scrollback work with bounded snapshots and stale-action rejection. | Pending: AccessKit consumer/geometry/action regressions, native VoiceOver task transcripts and Windows UI Automation evidence. |
| F06 | Scale the entire interface | Persisted user interface scale with keyboard commands and reset; all chrome, menus, dialogs, controls, trees and text scale consistently without double-scaling editor/terminal fonts. Maintain interaction geometry, accessibility bounds and usable layouts at 100/150/200 percent in English/Turkish, light/dark/contrast themes. | Pending: sizing/layout regressions, native theme/scale inspection and persistence checks. |
| F07 | Place views and terminals across windows | Primary/secondary sidebars with movable views, panel docking, terminals in editor groups, detached editors/terminal/view windows and transfer back. Shared documents and process ownership remain correct; focus, close/dirty prompts, persistence and multi-monitor geometry are safe. | Pending: ownership/window/restore tests and native multi-window interaction. |
| F08 | Preserve complete workspace configurations | Read/write named workspace files with common settings/tasks/launch, folder names and paths; well-defined user/workspace/folder precedence; all resource-scoped settings in additional roots. Preserve JSONC and unknown fields; relative paths, moves, missing roots, malformed files and trust boundaries are handled explicitly. | Pending: round-trip/precedence tests, multi-root GPUI and local/SSH task/debug behavior. |
| F09 | Manage multiple repositories together | Discover workspace/nested repositories and show a selectable inventory with branch, operation state and sync counts; aggregate changes without confusing identical relative paths. Stage/commit/sync/history target the selected repository; refreshes, removals, conflicts, trust and local/SSH boundaries preserve identity. | Pending: disposable multi-repo Git operations, stale-action failures, large-list rendering and native inventory flow. |
| F10 | Restore terminal sessions | Persist terminal profiles, cwd, titles, layout and bounded scrollback; reconnect to surviving processes on window reload and revive shells after app restart with explicit semantics. No accidental command replay; crashed sessions, changed hosts/profiles, corrupt state, privacy and cleanup handled. | Pending: agent/process-lifetime tests, restart/restore GPUI scenarios, native window reload/restart and leak checks. |
| F11 | Review and retain search results | Search Editor tabs with context, navigation, rerun and persistence; regular text replacement previews with per-match/file selection and diffs before applying. Preserve encoding, line endings, dirty buffers and undo; reject changed files, stale hosts, truncated results and cancellation safely. | Pending: actual replacement/undo/conflict tests, local/SSH searches, large-result budgets and native preview flow. |

## Delivery evidence

No feature has completed its PR acceptance yet. Keep acceptance requirements intact when evidence is missing.

### F01: implementation and local validation

- Two initial GPUI regressions failed on the baseline: keyboard range-copy omitted a selected
  file and select-all exposed only one selected row. Both pass with the implementation.
- `scripts/check.sh --strict` passed: formatting, Clippy, architecture/documentation/localization,
  license/security gates and 1,757 workspace tests; five existing opt-in tests remain ignored.
- Agent tests cover exclusive concurrent publication, recursive-copy limits and cleanup, binary
  files above the read limit, symlinks, executable permissions, cancellation and special files.
  Host protocol tests require cancellation acknowledgement and identify uncertain disconnects.
- GPUI tests cover mouse/keyboard selection, shrinking ranges, collapse/sort/refresh, stale read
  replies, duplicate descendants, cancelled/stale confirmations, partial failures/retry and dirty tabs.
  A 10,000-entry selection/render preparation took 14.4 ms in the local debug harness; this is
  a bounded fixture measurement, not a native frame-time claim.
- A separate native **Codeyard Bulk QA** bundle with disposable config/state selected `a.txt`
  and `b.txt` through real window keystrokes. macOS AX reported both selected, `b.txt` focused
  and `c.txt` unselected; a window-only screenshot confirmed the painted selection. A second
  walkthrough copied the two entries into `destination`, accepted the native Copy confirmation
  through AX, and verified both contents on disk with `c.txt` excluded.
- Windows CI includes the Explorer regression group. Required PR checks and memory/size budgets
  are pending. PR: https://github.com/doancan/codeyard/pull/48. New Turkish strings are drafted and pass localization checks; no human native-speaker
  review or real VoiceOver speech session is claimed.

The earlier Retina validation passed all memory scenes (74/69/94/84/116/76 MB, idle agent 1 MB).
A separate baseline-size investigation confirmed the executable-size breach already existed on the
baseline. Main subsequently adopted recorded startup/typing/memory comparisons and removed its app
size gate. This branch now follows that policy and keeps the existing thin-LTO release profile;
agent size limits remain unchanged. The latest merged-main candidate still needs its performance
comparison and repeated memory run.

Cross-platform validation found musl has no exported `renameat2` wrapper. Static agents now invoke
the same exclusive kernel operation directly; local x64/ARM64 builds and Linux CI pass. Native Windows
packaging passed. Expanded Windows Explorer coverage exposed non-normalized fixture paths and an
unbounded recursive-read retry after errors. Fixtures now use native-path normalization, read errors
stop recursion, and a regression asserts no replacement read is scheduled. Windows UI checks pass on `0293d1a`; macOS and Linux checks also pass. The complete required workflow is successful: Actions run `40766240680`. The daemon permission assertion now waits for its handshake readiness barrier.

Cancellation interrupts staged copies and stops before the next move/delete. A connection loss or
missing cancellation acknowledgement stops the batch with an explicit uncertain outcome; it never
automatically retries a mutation. Batches preserve failed/unprocessed cut entries for inspection or retry.

The Explorer candidate `0293d1a` passes all required platform checks (Actions run
`40766240680`) and a same-machine interleaved comparison against `c790c79`: 20 startup/typing
runs and five samples per memory scene. Startup p95 changed 238.3 → 236.8 ms (−0.6%),
typing p95 24.9 → 25.6 ms (+2.6%), and welcome/syntax/terminal memory medians changed
69 → 69 / 114 → 113 / 73 → 74 MB. Every comparison gate passes. The branch subsequently
integrates main's declarative-extension delivery; its integration head requires its own CI.

### F02: implementation in progress

- The storage regression initially reassigned groups 2/7/31 to group 1. Stable IDs now round-trip,
  and a rejecting SQLite trigger proves that layout-write failure rolls back the tab update too.
- Nine layout tests cover splitting in all directions, spatial neighbors, stable identities,
  bounded/corrupt input, ratio validation and 500 mixed operations preserving a complete partition.
- GPUI coverage exercises three views of one shared document, independent cursors and undo,
  nested geometry and resizing, stale drag/drop and late file completion, background-load focus,
  restart restoration, empty groups and group-local settings pages. A 32-group workload exercises
  minimum dimensions, focus scrolling and the unchanged group limit after another split request.
- The first integration tests found an empty-session restore could cancel a command-line file
  opening. Empty legacy sessions now leave that request intact. Group lifetime tokens prevent a
  late completion from populating a newly created group with a reused slot number.
- Full regression, native-window acceptance, memory/size and required PR checks remain pending.

The 32-group navigation regression initially skipped siblings after group 25 because normalized
coordinates rounded together. Directional navigation now uses minimum-extent geometry. Native pane
placement is calculated once and each visible pane is laid out independently; fully clipped panes
keep their views without being rendered. This also removes repeated intrinsic-size measurement
through deeply nested flex regions. The debug workload that creates 32 highlighted 10k-line views
and focuses left 31 times improved from 75.4 seconds to 9.1 seconds without relaxing its 15-second
local budget (CI's existing timeout scale applies). The related random-command torture test passes.
PR: https://github.com/doancan/codeyard/pull/56. Native acceptance and final gates remain pending.

The latest F02 hardening pass adds three baseline-failing regressions: resizing a window must
keep the focused document visible, opening a saved file from the command line must preserve all
groups and its unsaved hot-exit backup, and narrow groups must retain room for text. The latter
initially left only 7.5 px for text with the minimap enabled. Each group now hides its minimap
below 320 px and restores it when widened. The 32-view highlighted workload took 3.4 seconds
in the focused run, within the existing 15-second local limit.

`CODEYARD_REQUIRE_SSHD=1 scripts/check.sh --strict` passes on these changes: 1,850 tests,
zero failures and five existing opt-in ignores. Prior Retina measurements on the flat renderer
passed at 73/73/93/83/114/79 MB for terminal/welcome/small Rust/plain 10k/highlighted 10k/first-run;
the idle agent used 1 MB. Static x64/ARM64 agents were 1,576/1,372 KiB.

An isolated native QA bundle created a four-group grid through the menu and command palette.
Typing in two groups and saving produced both edits in the same file. Restarting with that file
retained the four-group topology, focus and per-group views. Native Turkish narrow-window and
wide-window captures were inspected; the final minimap correction is confirmed in a native 480 px Turkish/light window and restores the minimap after widening.
All CI checks on the earlier `326612b` candidate pass (Actions run `40766365499`); the final
hardening commit must pass its own workflow before merge. No real VoiceOver speech or
human translation review is claimed.

The final pre-integration F02 Retina budget rerun passes at 72/69/93/83/113/69 MB (terminal/welcome/small Rust/plain 10k/highlighted 10k/first-run), with a 1 MB idle agent and unchanged static-agent sizes. Main integration preserves the new language key context and the per-group minimap behavior; its head requires fresh regression and CI checks.

The integrated editor-grid source passes `CODEYARD_REQUIRE_SSHD=1 scripts/check.sh --strict`:
1,885 tests, zero failures, five existing opt-in ignores. A locale-merge error joined four comment
headers to following rows; restoring their line boundaries fixes all 31 localization regressions.

The pre-integration grid candidate `27951f3` also passes the same-machine interleaved comparison
against Explorer `0293d1a` (20 startup/typing runs, five memory samples per scene): startup p95
236.5 → 233.1 ms (−1.4%), typing p95 26.0 → 25.7 ms (−1.0%), and welcome/syntax/terminal
medians 69 → 69 / 114 → 114 / 74 → 73 MB. The current integrated sources are receiving a
separate comparison against the updated shared main reference. Required CI on the integration
head remains a merge condition.

### F04: task engine foundation

The UI-independent `task-engine` crate parses bounded JSONC task definitions, preserves literal
process arguments and platform/environment options, validates reachable dependency graphs and
constructs parallel or sequential barriers without duplicating shared prerequisites. Expanded
inherited configuration is capped at 8 MiB to prevent a small file multiplying into large state.
The scheduler reserves each command once, requires watch readiness before releasing dependents,
propagates failures to running dependents and bounds concurrent startup/live processes.

Variable resolution uses the file host's supplied environment, named workspace roots and one pass
over the original template. Input/command results are resolved by the caller once and never parsed
again as variable templates. These planning components pass 20 hermetic tests and strict Clippy.
Process transport, custom matchers, input UI, task/debug integration and native acceptance remain
unfinished; this foundation is not a completed task-execution delivery.

Two real-process regressions fail before their fixes: a descendant survives cancellation after its
leader exits, and a process that closes stdout makes cancellation wait 4.9 seconds for natural exit.
Cancellation now kills the owned group before reaping its leader, and output EOF polls exit without
removing a still-running process from its session. The regressions pass; 182 agent/host/task-engine
tests and strict Clippy pass. Windows process-tree ownership and the remaining F04 acceptance are pending.

Windows piped processes now start suspended, enter a private job with kill-on-close ownership,
and resume only after assignment. Native Windows fixtures cover live and already-exited leaders;
their actual execution remains a CI condition. Windows GNU cross-target all-target Clippy passes,
and the existing 182 macOS agent/host/engine regressions still pass.

The matcher core supports custom consecutive-line patterns, repeating final lines, file/range
captures, built-in templates and overrides, and file-scoped watch readiness. It validates capture
indices and required fields before execution, shares a regex compilation budget across a plan,
and bounds regex size, delegated automata and backtracking. Lookarounds/backreferences reuse the
already-locked regex implementation. An explicit empty matcher list disables automatic detection.
The engine's 28 tests and strict Clippy pass. Byte-stream decoding, workbench diagnostics and
runtime readiness integration remain pending.
