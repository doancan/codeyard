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
| F01 | Select and manage multiple Explorer entries | Modifier-toggle and Shift-range mouse selection; keyboard range/select-all; stable path selection through refresh, sorting and collapse; selected-row semantics. Copy, cut, drag, paste and delete operate on the selection with one reviewable summary, parent/child deduplication, root protection, no overwrites, per-entry failures, retryable failed cuts, preserved dirty tabs and stale-host guards. File copying works through the agent without shell-specific commands. | Pending: selection, filesystem, GPUI and SSH regressions; native interaction; required PR checks. |
| F02 | Arrange editors in a persistent grid | Arbitrary nested horizontal/vertical groups; split in each direction; resize, move/reorder tabs, focus/navigation and close empty groups; shared document/independent views. Persist and validate layouts across restart, workspace changes, narrow windows and many groups; migrate existing two-group sessions. | Pending: layout model/property cases, GPUI editing/restore tests, native layout and memory measurements. |
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
stop recursion, and a regression asserts no replacement read is scheduled. Windows UI checks are
being rerun. The daemon permission assertion now waits for its handshake readiness barrier.

Cancellation interrupts staged copies and stops before the next move/delete. A connection loss or
missing cancellation acknowledgement stops the batch with an explicit uncertain outcome; it never
automatically retries a mutation. Batches preserve failed/unprocessed cut entries for inspection or retry.
