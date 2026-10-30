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
| F01 | Select and manage multiple Explorer entries | Modifier-toggle and Shift-range mouse selection; keyboard range/select-all; stable path selection through refresh, sorting and collapse; selected-row semantics. Copy, cut, drag, paste and delete operate on the selection with one reviewable summary, parent/child deduplication, root protection, no overwrites, per-entry failures, retryable failed cuts, preserved dirty tabs and stale-host guards. File copying works through the agent without shell-specific commands. | Accepted for delivery in [PR #48](https://github.com/doancan/codeyard/pull/48). Final product `9d19214`; required platform CI run `40792799645` passes. Filesystem/protocol/GPUI/SSH tests, native batch-copy acceptance and final performance/size gates pass; see the recorded evidence below. |
| F02 | Arrange editors in a persistent grid | Arbitrary nested horizontal/vertical groups; split in each direction; resize, move/reorder tabs, focus/navigation and close empty groups; shared document/independent views. Persist and validate layouts across restart, workspace changes, narrow windows and many groups; migrate existing two-group sessions. | Accepted for delivery in [PR #56](https://github.com/doancan/codeyard/pull/56). Final product `0b39890`; required platform CI run `40793024519` passes. Native shared-edit/restart/narrow-window acceptance, layout/storage/GPUI tests and final performance/size gates pass. |
| F03 | Set up language services from the editor | Discover bundled/installed JS/TS, HTML and CSS services; an explicit, discoverable install/retry flow with progress, cancellation, exact destination, offline errors and version/integrity controls. Install on the file's host; respect workspace trust and explicit user commands. Verify completion, diagnostics, formatting and restart without a terminal setup step. | Accepted for delivery in [PR #60](https://github.com/doancan/codeyard/pull/60). Final product `7bfff5c`; required platform CI run `40793025924` passes. Actual local/SSH installation and language interactions, native install/cancel/reinstall/restart, failure/geometry/action tests and final Retina/performance gates pass. |
| F04 | Execute dependency-aware tasks | Typed shell/process tasks, cwd/env/platform options, variable/input expansion, parallel/sequence dependencies and cycle validation; background/watch readiness and custom problem matchers; pre/post-debug integration. Bounded output, cancellation of the process tree, concurrency policy, clear diagnostics and correct failed dependency behavior. | Foundation in [PR #61](https://github.com/doancan/codeyard/pull/61). End-to-end execution (typed `TaskSpawn`, protocol 15), inputs, per-command output terminals, Stop/Restart, matcher Problems and preLaunchTask gating are implemented on `feature/76-task-graph` with agent, host, runner and GPUI tests. Pending: Windows process-tree and quoting evidence, native keyboard walkthrough in English/Turkish (SSH is covered by a test on the throwaway sshd). |
| F05 | Complete accessible application workflows | Search/results, Problems/fixes, Settings/shortcuts, debug/testing/remote views, SCM graph/worktrees/sync, output/ports, menus and dialogs expose roles, states, actions, errors and focus restoration. Row/control/text geometry, document-relative text selection, scroll actions and terminal scrollback work with bounded snapshots and stale-action rejection. | In progress on `feature/77-accessible-problems`: Problems panel list, empty state and guarded activation; bottom panel tabs and Output; search sidebar; Settings page, Keyboard Shortcuts page and Ports list with guarded actions; Run and Debug view and debug toolbar, Testing view, Remote Explorer and context menus with guarded actions. On `feature/77-accessible-dialogs-menus`: the extension consent sheet as a modal alert dialog with guarded Install/Cancel and focus restoration, row menus of Run and Debug, Remote Explorer and Ports rows through `ShowContextMenu` and a "More Actions…" button with stale-row rejection, painted, clipped row bounds for the Run and Debug, Testing, Remote Explorer and Ports lists, and keyboard rows in those lists (Tab, Up/Down/Home/End, Enter, focus ring) with a rebindable Shift+F10 / Menu key context-menu command shared with the Explorer and the editor. Pending: data breakpoints, bounds of Settings/Shortcuts rows and menu items, native VoiceOver transcripts and Windows UI Automation evidence. |
| F06 | Scale the entire interface | Persisted user interface scale with keyboard commands and reset; all chrome, menus, dialogs, controls, trees and text scale consistently without double-scaling editor/terminal fonts. Maintain interaction geometry, accessibility bounds and usable layouts at 100/150/200 percent in English/Turkish, light/dark/contrast themes. | Pending: sizing/layout regressions, native theme/scale inspection and persistence checks. |
| F07 | Place views and terminals across windows | Primary/secondary sidebars with movable views, panel docking, terminals in editor groups, detached editors/terminal/view windows and transfer back. Shared documents and process ownership remain correct; focus, close/dirty prompts, persistence and multi-monitor geometry are safe. | Partly implemented on `feature/79-movable-views`: secondary side bar for every side view (persisted), terminal into its own window or into an editor group and back with one owner and the same process (closing its window or tab returns it running); terminal tabs drag between groups and are in the accessibility tab list; after a restart, editor-area and detached terminals return in place as new shells (versioned, bounded layout; nothing replayed; detached windows clamped onto an attached display, damaged layouts ignored). Pending: detached editors and side views, panel docking, drag and drop of views between side bars, SSH/Windows native verification. |
| F08 | Preserve complete workspace configurations | Read/write named workspace files with common settings/tasks/launch, folder names and paths; well-defined user/workspace/folder precedence; all resource-scoped settings in additional roots. Preserve JSONC and unknown fields; relative paths, moves, missing roots, malformed files and trust boundaries are handled explicitly. | Pending: round-trip/precedence tests, multi-root GPUI and local/SSH task/debug behavior. |
| F09 | Manage multiple repositories together | Discover workspace/nested repositories and show a selectable inventory with branch, operation state and sync counts; aggregate changes without confusing identical relative paths. Stage/commit/sync/history target the selected repository; refreshes, removals, conflicts, trust and local/SSH boundaries preserve identity. | Pending: disposable multi-repo Git operations, stale-action failures, large-list rendering and native inventory flow. |
| F10 | Restore terminal sessions | Persist terminal profiles, cwd, titles, layout and bounded scrollback; reconnect to surviving processes on window reload and revive shells after app restart with explicit semantics. No accidental command replay; crashed sessions, changed hosts/profiles, corrupt state, privacy and cleanup handled. | Restart revival implemented on `feature/82-terminal-restore`: versioned bounded metadata, new shells with no replay, sanitized opt-in history, missing-folder fallback, damaged-state recovery, clear command; GPUI tests with real shells. Local reconnection on `feature/82-local-reattach`: panel terminals of local folders run on the terminal daemon and are taken back after an app restart or crash with host-, instance- and terminal-scoped tokens (never a pid), single owner per session, bounded buffered output with a counted gap, orphan and idle timeouts; real-process daemon, host and GPUI tests. Pending: taking back terminals moved out of the panel, SSH sessions across an app restart, Windows, native walkthrough. |
| F11 | Review and retain search results | Search Editor tabs with context, navigation, rerun and persistence; regular text replacement previews with per-match/file selection and diffs before applying. Preserve encoding, line endings, dirty buffers and undo; reject changed files, stale hosts, truncated results and cancellation safely. | Pending: actual replacement/undo/conflict tests, local/SSH searches, large-result budgets and native preview flow. |

## Delivery evidence

F01–F03 satisfy the acceptance gates below. F04–F11 remain incomplete. The user has
authorized merging the validated task foundation while the remaining workflows receive
separate feature specifications. The three explicitly excluded areas remain outside scope.
Product sources were measured at the exact commits in the table; subsequent changes to this
ledger and the benchmark archive contain evidence only.

### Final integrated checks

- Main reference: `0502dd48852431e67ee0a15af69b393a5482bdb6`, including Web Preview and the
  installed-extension startup scenario. Required macOS, Windows and Linux workflows pass on
  each proposed product head: [F01](https://github.com/doancan/codeyard/actions/runs/40792799645),
  [F02](https://github.com/doancan/codeyard/actions/runs/40793024519), and
  [F03](https://github.com/doancan/codeyard/actions/runs/40793025924).
- `CODEYARD_REQUIRE_SSHD=1 scripts/check.sh --strict` passes on the integrated grid/service
  sources: 1,930/1,950 tests, zero failures, five/six documented opt-in ignores, no missing-tool
  skips. Formatting, strict Clippy, architecture, docs, localization, licences, security and real
  SSH checks pass. The final shared response-serializer change subsequently passes all 175
  agent/host/protocol tests, strict Clippy and the complete required platform CI above.
- The real SSH suites run serially because automatic host port discovery can observe other
  concurrent suites. No assertion or timeout was weakened. Explicit SSH configuration paths
  now namespace ControlMaster sockets; a regression covers distinct and non-UTF-8 paths.
- Web Preview retains protocol tag 29. Agent Copy uses appended tag 30 and protocol version 13,
  with golden encoding and mutated-frame coverage. Release profiles and size limits are unchanged.
  Static x64/ARM64 Linux agents are 1,600/1,392 KiB; the 1,600 KiB ceiling passes. Sharing the
  borrowed response codec for live, replayed and daemon replies avoids duplicate serialization code.
- The final F03 native Retina memory pass reports terminal/welcome/small Rust/plain 10k/
  highlighted 10k/first-run footprints of 74/69/93/83/114/77 MB, all within the existing budgets.
  The idle agent uses 1 MB against its 8 MB budget. App binary size is informational under main's
  existing policy.

### Same-machine performance acceptance

The four exact product builds ran in one quiet, rotating interleaved comparison: 20 runs per
timing scenario, 40 drawn/saved keys per typing run (800 samples per build), and five samples per
memory scene. Every known build must produce a first frame; every typing run must draw and save
all 40 keys. The extension scenario installs 100 inactive extensions once into an isolated profile;
only that scenario contributes extension-load samples. An isolated small QA window and an owned
keep-awake process keep the measured windows visible on the same desktop Space.

The runner uses `scripts/perf/run.sh` with its build phase replaced by copies of prebuilt release
binaries, four-way rotating order and checkpoint copies. Measurement workloads and thresholds
are unchanged. [The exact runner](benchmarks/core-workflows-2026-10-29/run-prebuilt.sh),
[baseline samples](benchmarks/core-workflows-2026-10-29/baseline.json),
[F01 samples](benchmarks/core-workflows-2026-10-29/explorer.json),
[F02 samples](benchmarks/core-workflows-2026-10-29/editor-grid.json) and
[F03 samples](benchmarks/core-workflows-2026-10-29/language-services.json) are retained.
Each row compares with the preceding row, with F01 compared to main `0502dd4`. Memory columns
are welcome/highlighted 10k/terminal medians. All blocking metrics and the informational window-open
metric pass the unmodified catalogue. In particular, loading 100 extensions stays below 5 ms.

| Feature | Product source | Startup p95 | Typing p95 | 100 extensions p95 | Memory medians | Result |
| --- | --- | --- | --- | --- | --- | --- |
| F01 | `9d19214` | 208.2 ms (-0.5%) | 26.0 ms (+2.9%) | 0.971 ms | 69/114/73 MB | Pass |
| F02 | `0b39890` | 217.4 ms (+4.5%) | 26.0 ms (+0.0%) | 0.967 ms | 69/114/74 MB | Pass |
| F03 | `7bfff5c` | 211.2 ms (-2.9%) | 25.5 ms (-1.9%) | 0.918 ms | 69/114/74 MB | Pass |

Run `python3 -I scripts/perf/compare.py` on each adjacent archived pair to reproduce the results.
Saved comparison outputs are [F01](benchmarks/core-workflows-2026-10-29/explorer-comparison.txt),
[F02](benchmarks/core-workflows-2026-10-29/editor-grid-comparison.txt) and
[F03](benchmarks/core-workflows-2026-10-29/language-services-comparison.txt).

### F01: Explorer selection and batch operations

- Modifier/range mouse and keyboard selection survive refresh, sorting and collapse. Initial
  baseline-failing regressions cover omitted range-copy entries and incorrect selected-row semantics.
- Batch copy/cut/move/drag/delete use one confirmation, bounded progress, cancellation, parent/child
  deduplication, protected roots, per-entry errors and retryable failed cuts. Dirty tabs are retained;
  stale host/root replies and uncertain mutation outcomes stop without an automatic retry.
- Agent filesystem tests cover streamed large files, recursive limits, symlinks, executable modes,
  exclusive concurrent publication and staging cleanup. Native Windows paths, read-only files and
  directory symlinks are included in CI. Recursive read errors no longer schedule an endless retry.
- An isolated native macOS walkthrough selected two files using real keystrokes, confirmed both
  selected rows and focus through AX and the painted window, invoked one Copy confirmation through
  AX, and verified the two destination contents while the unselected file remained absent.

### F02: persistent editor grids

- A validated binary layout supports up to 32 stable groups, four split directions, divider resizing,
  directional focus and tab transfers. Views share a document and undo history while retaining
  independent cursor, scroll, fold and find state; lifetime tokens reject stale completions and drags.
- Layout and tabs commit in one SQLite transaction, including rollback on a forced write failure.
  Existing sessions migrate, empty groups restore, and command-line reopening coalesces with session
  restore while preserving unsaved hot-exit backups.
- Baseline-failing resize, CLI restore, narrow minimap and late-group navigation regressions pass.
  Minimap rendering stops below 320 px per group and returns when widened. Flat layout renders each
  visible pane once and keeps clipped views without painting them. The focused 32-view highlighted
  10k-line navigation workload took 3.4 seconds in debug, within the unchanged 15-second local bound;
  mixed-command torture and 500 layout operations preserve the partition.
- Native acceptance created four groups through the menu/palette, edited one shared file through
  two groups and verified both saved edits. Restart retained topology, focus and views. Final native
  480 px Turkish/light and wide-window checks confirmed text space and minimap restoration.

### F03: managed language services

- An explicit Language Services page exposes the current host, exact versions, destination,
  bounded progress/logs, retry and cancellation. Opening the page only discovers installations.
  The same worker installs on local or SSH hosts; explicit commands, disabled languages and trust
  retain their existing behavior.
- TypeScript 6.0.3, TypeScript Language Server 6.0.2 and extracted HTML/CSS services 4.10.0 use a
  validated 35-package lock, registry-only HTTPS URLs and SHA-512 integrity. A compatible Node/npm
  is reused; otherwise pinned Node 22.23.4 archives are SHA-256 verified before execution. npm uses
  private configuration/cache and disables lifecycle scripts, audit, funding and update notices.
- Private staging and immutable generations activate through an atomic active record. Failures and
  cancellation preserve the previous record; uncertain host outcomes retain staging. Cancellation
  waits for the agent's kill acknowledgement, proven by a real owned-process regression. Local
  profile overrides also isolate service storage; remote destinations use the remote host's home.
- Real local installation downloaded, verified, installed, published and rediscovered the services.
  A separate owned SSH fixture performed actual installation. Codeyard's real JSON-RPC client then
  received TS/HTML/CSS completions (44/116/888), formatting edits (9/1/1) and one diagnostic each
  on both local and SSH hosts, including Unicode paths and advertised formatter capabilities.
  Regressions fix null HTML/CSS configuration and a disabled default formatter while preserving
  explicit options.
- Native first-run acceptance installed successfully, cancelled a fresh attempt without activation,
  cancelled a reinstall while preserving byte-identical active state and one completed generation,
  then restarted and rediscovered Ready. English/dark wide and Turkish/light 480 px captures were
  inspected. Controls stay above scrollable details, expose clipped AX bounds, support keyboard
  actions/live status and reject stale page, revision and workbench actions.

### Verification limits and remaining work

These are scoped acceptance results, not claims of universal platform or accessibility coverage.
Real installer and native interaction walkthroughs ran on macOS; local and disposable SSH language
tests used the actual services. Windows/Linux required CI passes, but a real Windows/Linux network
installation walkthrough, real VoiceOver speech and human translation review were not performed.
The managed Linux download needs glibc; musl hosts must supply compatible Node/npm as documented.

F04's foundation is reviewed and validated separately from its unfinished user workflow. F05 accessibility across the whole app,
F06 interface scale, F07 movable/multi-window views, F08 complete workspace configurations,
F09 multiple repositories, F10 terminal session restoration and F11 retained search/replacement
review remain pending. Their feature issues must remain open until their complete acceptance criteria pass.

### F04 foundation scope

The task-engine crate provides bounded JSONC/variable/DAG planning, scheduling, watch readiness,
custom matcher models and chunked output decoding, with 34 component tests. Process ownership
keeps cancellation available after output closes and kills Unix groups or owned Windows jobs.
The workbench still uses its existing task workflow; no task UI or transport wiring is implied.
Windows container-runner tests use owned child fixtures instead of depending on cold PowerShell
startup. Their 10-second success deadline and 100 ms timeout assertion remain unchanged.
