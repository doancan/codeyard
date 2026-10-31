# Product quality review and development plan

Review date: 2026-10-28. Platform: macOS, Apple silicon, native GPUI window at 1280 × 800 logical pixels.
Baseline: `main` at `7fbeae2`, merged into `develop` at `94a7ae2`, preserving develop's web preview,
container overview and SSH/Cargo artifact fixes. Main receives these changes through a pull request after its checks pass.
The concurrent upstream develop commit `2ea3ad8` was integrated at `f5dcdb2`, preserving its container refinements and landmark geometry.
The later `7a029e6` Git Worktrees inventory/navigation change was fetched and retained as well; its 11 focused tests passed locally.

This is an evidence-based backlog, not a claim that every workflow or platform is certified. Findings distinguish
native-window observations, executable regressions, implementation limitations and work that still needs measurement.
The existing interface, native conventions, semantic tokens and Turkish translation system are retained.

## Scope and evidence

The walkthrough uses a separate **Codeyard QA** bundle, disposable settings/state and a generated Rust repository.
Its Git remote is another temporary directory. The user's running editor, profile, repositories and real remotes
are not used for destructive tests. No real SSH host is contacted by the walkthrough; the suite supplies test SSH servers.

| Area | Evidence collected | Remaining acceptance work |
| --- | --- | --- |
| Editor and navigation | Native quick open, Rust syntax, tab and text exposure; workspace editor regression suite | IME, RTL, mixed DPI and long sessions on real Windows hardware |
| Explorer | Native keyboard navigation; expanded empty-folder regression; path-based accessibility actions | Multi-selection, bulk operations and accessibility beyond the row limit |
| Git | Native Source Control; disposable index/commit action tests; existing branch, sync, conflict and local-remote tests | Accessible graph, repository selection and large change sets |
| Rust IDE | Native server-independent manifest lookup and real program/test debugging; actual virtual-workspace, named-target and required-feature builds | Unified user-selected feature/profile settings, macros/module remapping and the real SSH/Windows adapter matrix |
| Customization | Existing settings catalog, JSONC, keybinding/import and theme tests | Full-interface scaling and a discoverable Rust configuration preset |
| Accessibility | Real macOS AX tree; AccessKit consumer validation; focus/action regression tests | VoiceOver speech/rotor, bounds, other panels/dialogs and real Windows UI Automation |
| Reliability/performance | Build, Clippy, architecture/docs/i18n gates, workspace tests and final Retina memory/size budgets | Startup/frame timings, long sessions and active-AT CPU cost |

## Verified findings

Severity: P0 blocks ordinary use or loses data; P1 blocks a user group or a core workflow; P2 impairs productivity
with a workaround; P3 is cosmetic. No P0 was reproduced in this walkthrough.

| ID | Priority / evidence | Finding and user impact | Location | Acceptance criterion |
| --- | --- | --- | --- | --- |
| Q01 | P1 · done (#201) | Source Control is in the AccessKit tree used by both the macOS and Windows adapters (also in a window of its own): branch, editable message, commit button with its state ("Nothing to commit" when disabled), a "Changed files" list of groups with header buttons, and per file a row that opens its diff plus Stage/Unstage (Mark as resolved) and Discard (behind the existing question). The list exposes 500 lines around the keyboard cursor or the visible rows with scroll actions; the new keyboard cursor of the change list is the focused node; errors are assertive alerts. Actions reuse the Git operations and are refused for another repository/branch or a file no longer in the same group and state. Checked natively with an AX dump and AXPress/AXFocused on a dev build. | `crates/app/src/a11y.rs`, `scm.rs` | Expose the visible panel, message, commit state, change groups, per-file actions, operation errors and focus. Actions reuse the existing Git operations and reject stale repository/file state. |
| Q02 | P2 · done (#202) | Explorer leaves the AX tree whenever no side bar draws it: another view, a hidden side bar, and now zen mode with the Explorer in the secondary side bar (it used to stay there). Focus, scroll and row actions from an older tree do nothing on the hidden Explorer. | `crates/app/src/a11y.rs` | Only expose Explorer while its sidebar is visible; stale tree actions cannot operate on a hidden sidebar. |
| Q03 | P2 · done (#204) | Already fixed in 0.6.0: Right Arrow on an expanded folder enters a child only when a visible child exists. Tests now cover open folders that are empty, loading, unreadable or non-empty, a closed folder and a file, and the accessibility focus; they fail with the old next-row behavior. | `crates/app/src/tree.rs` | Enter a child only when a visible child exists; empty, loading and nonempty folders retain correct keyboard behavior. |
| Q04 | P2 · done (#204 tests) | Rust commands name the case: a non-Rust file ("Open a Rust file first"), disabled language features (`lsp.enabled`), a restricted workspace (trust the folder) and an unavailable server (restart language servers). "Rust: Open Cargo.toml" finds the nearest manifest, inside the workspace, without a server. Regression tests cover all four notices and the server-less manifest lookup. | `crates/app/src/workbench_rust.rs` | Distinguish a non-Rust file, disabled features, restricted workspace and unavailable server. Opening the nearest manifest should work without a language server. |
| Q05 | P1 · implementation + documented limitation; landmarks integrated | Window, primary Explorer, focused editor and status-bar landmarks now have bounds. Individual rows, controls and text positions still lack the geometry needed for precise assistive cursor/caret tracking. | `crates/app/src/a11y.rs`, `a11y_geometry.rs`, `docs/accessibility.md` | Extend painted bounds to controls and shaped text positions, then verify VoiceOver and Windows focus tracking in real windows. |
| Q06 | P1 · implementation + documented limitation | Search, Problems, Settings, debug/testing views, context menus and most dialogs lack complete AX representations. Keyboard shortcuts do not replace screen-reader semantics. | `docs/accessibility.md`, the corresponding view modules | Cover search→result→diagnostic→fix, settings editing and every confirmation/cancellation path, with focus restoration and live errors. |
| Q07 | P2 · done (#196) | Cargo metadata discovers the targets of a workspace or package, including virtual workspaces and binaries named differently from their package; with more than one, F5 offers a picker whose rows also say how the target will be built. Run and Debug build the picked target with `rust.features`, `rust.noDefaultFeatures`, `rust.profile` and `rust.targetDir`, and take the program from Cargo's own artifact report, so a custom output folder (setting or `.cargo/config.toml`) works locally and over SSH. Workbench tests build a real two-binary workspace with a custom profile and output folder, locally and on an SSH host. | `crates/app/src/debug_config.rs`, `debug_cargo.rs`, `cargo_project.rs`, `settings_rust.rs` | Discover workspace/package targets, offer a picker and build the selected target with the chosen features/profile; respect custom target directories locally and over SSH. |
| Q08 | P2 · implemented; native walkthrough pending | Zoom scales the whole interface (`ui.scale`, 75–200%, persisted, every window, reset to 100%) at one GPUI boundary, including hit regions and accessibility bounds; editor and terminal font sizes keep their values and are scaled once. Regression tests cover 100/150/200%, a 480-point window at 200% in English and Turkish, drags, menus, IME and display changes. Not yet checked natively across the theme/language matrix or with a screen reader. | `vendor/gpui/src/ui_scale.rs`, `crates/app/src/workbench_panels.rs`, `crates/app/src/ui_scale_tests.rs`, `docs/accessibility.md` | Native walkthrough at 100/150/200% (dark, light, contrast; English, Turkish), VoiceOver/NVDA at 150/200%, and a monitor-DPI move. |
| Q09 | P2 · done (#190) | Measured in a debug build: drawing the panel took ~240 ms with 1k changes and ~2.3 s with 10k, because every change row was built. The single-repository change list is now a uniform list that builds only the visible rows (~6 ms for 1k and 10k); its scroll position and the keyboard focus survive a status refresh, and another repository starts at the top. Regression tests bound the frame time for 1k/10k changes. | `crates/app/src/scm.rs`, `scm_changes.rs`, `scm_changes_tests.rs` | Benchmark 1k/10k changes, virtualize rows if the frame budget is exceeded, and preserve focus/scroll position through refresh. |
| Q10 | P2 · done (#195); documented limitation | The Explorer exposes a range of 1,000 rows and the editor a range of 300 lines, centred on the selection or caret while it is on screen, otherwise on the visible area. Both report their scroll position and support scroll up/down by a page, which moves the range; set positions, run offsets and the selection are relative to the whole tree or document. Snapshots stay bounded. The editor's text value still starts at the range (see `docs/accessibility.md`). | `crates/app/src/a11y.rs`, `tree.rs`, `editor_io.rs`, `a11y_range_tests.rs` | Expose a selection-centered/visible range with scroll actions and document-relative text offsets; keep snapshots bounded. |
| Q11 | P2 · done (#206) | Explorer multi-selection (Shift-range, platform-modifier toggle, Select All) and bulk copy/cut-paste/move/delete: one confirmation lists the entries (12 shown, the full list in Output), dirty buffers stay open, and the result reports completed, failed and unprocessed entries with each failure in Output. Over SSH every operation goes through the remote agent (exclusive copy and rename, so nothing is overwritten); a delete there asks "Permanently delete N items on <host>?", says the host has no Trash and its button is *Delete Permanently*. Tests against the throwaway `sshd` run a bulk copy and move with a taken name, cancel before confirming, and a permanent delete with a file that cannot be removed. | `crates/app/src/tree.rs`, `workbench_fsops.rs` | Add Shift-range and modifier-toggle selection, bulk copy/move/trash with one reviewable summary and per-file failures, locally and over SSH. |
| Q12 | P2 · done (#200) | "Preferences: Apply Rust Setup…" writes an optional Rust setup to `settings.jsonc`: Clippy for checks, checks with the Cargo features Run and Debug build with (`rust.features`, `rust.noDefaultFeatures`), a separate check folder inside the target directory, and rustfmt through rust-analyzer on save. It first shows the exact JSONC diff and writes only after confirmation (and only if the file did not change meanwhile); comments and unrelated keys are kept. "Preferences: Reset Rust Setup…" removes exactly those keys, restoring the original text. | `crates/app/src/workbench_settings.rs`, `settings_templates.rs`, `settings_text.rs`, `rust_setup_tests.rs` | Add an optional Rust setup preset for Clippy, Cargo features, formatter and target directory; preview the exact JSONC changes, preserve unrelated comments, allow reset. |

## Delivery order

The first batch now implements Q01–Q04 and the target-discovery part of Q07. The additional Rust hardening request
promoted the following work ahead of the remaining UI backlog:

| Rust finding | Delivered behavior | Regression evidence |
| --- | --- | --- |
| A root package name was treated as a binary name | Trusted, offline Cargo metadata supplies actual workspace targets, examples, required features and library-only test harnesses | A real virtual workspace builds and runs a differently named binary in a custom output folder; its artifact also reaches a scripted DAP session |
| Debug could compile old on-disk buffers | Open files in the active root save before discovery/build/start; a save error, conflict, Stop or workspace change cancels the pending start | The Cargo→DAP test edits an unsaved buffer; a separate conflict test preserves an external write; Stop prevents the build |
| Stop did not invalidate pending test-build/discovery replies or late startup failures | Preparation replies carry a cancellation epoch, and ended sessions reject late attachment, errors and stopped events | A real queued Cargo test build cannot start after Stop; a cancelled pre-launch command cannot post its late failure |
| Async attributes with parameters and raw function identifiers were missed | The planner recognizes these test declarations | The old implementation failed the discovery regression |
| Debug Test at Cursor selected the preceding test from an unrelated helper | It uses the same cursor planner as Run Test | The old implementation failed the helper-function regression |
| A failed test build could still have emitted executable artifacts | Exit status and Cargo's build-finished message must both allow the launch; empty/duplicate artifacts are removed | Partial-artifact, failed-exit and missing-exit regressions; an actual failing Cargo build populates Problems and starts no session |
| Debug selectors lost manifest paths, target flags or features | Quoted test commands are decoded to literal argv and Cargo is invoked directly | Paths with spaces, quotes, shell metacharacters and Turkish text round-trip without shell evaluation |
| Windows compiler paths did not match editor paths; panic locations rejected drive letters/spaces | Windows drive/UNC diagnostic paths normalize to editor paths, while POSIX backslashes remain literal | Expanded native Windows CI caught the real failed-build mismatch; two portable compiler/panic path regressions failed before the fix |
| Directory names were guessed as package names | Cursor/file/Testing-view commands use the nearest actual manifest and crate-relative module path | A member under packages/folder name keeps its nested test ID and manifest path |
| Repeated test IDs could overwrite failures, or a failed build could leave an old pass visible | Failure wins for duplicate IDs with an attribution warning; missing results from failed runs replace earlier green results | Both output orders and an unreported failed-build regression; Run All uses --no-fail-fast |

The Rust first-batch tests are in `rust_hardening_tests.rs` and `cargo_project_tests.rs`; the initial product tests are
in `product_quality_tests.rs`. These are actionable boundaries, not a claim that every Rust macro/test framework is supported.

### Pass 1: repair the reproduced daily-use barriers

Q01–Q04, stale accessibility-row safety and stale Rust-result guards form the first implementation batch.
Add a bounded manifest lookup for **Rust: Open Cargo.toml** when no Rust server is attached. It must stay inside
the containing workspace, choose a member crate before the workspace manifest, use the file's host and never
read upward through unrelated parent folders. An out-of-date reply must not open a file in a new workspace.

Use meaningful regressions before fixes, retain local/SSH behavior through `Host`, add Turkish strings and document
the changed behavior. Source Control's graph, worktree accessibility, sync controls and full keyboard traversal remain separate work;
this pass establishes accessible change inspection, staging and committing.

### Pass 2: complete the accessible task chain

Q05 and Q06: extend the integrated landmark bounds to controls/text, then Search, Problems and Settings, followed by input/confirmation/context
menus. Each view needs semantic roles, state, actionable errors, predictable reading order and focus restoration.
Test the complete task with VoiceOver on macOS and NVDA/Narrator on Windows. AX dumps alone do not certify speech.
Include the native interface-scaling walkthrough (Q08) and bounded visible-range navigation (Q10) before claiming broad accessibility.

### Pass 3: remaining Rust project workflows and developer customization

Core target discovery shipped in Pass 1. Continue Q07 with a unified user-selected features/profile model shared by
Run, Debug and Testing; cover explicit module remapping, macro-generated cases and cross-root path dependencies.
Extend the actual Cargo/adapter acceptance matrix over a test SSH host and native Windows.
The new Cargo/DAP, product and accessibility regressions are also selected by Windows CI; real hardware/adapter
acceptance remains separate from these native automated fixtures.

Q12: expose presets through existing settings and tasks rather than adding another configuration format.
Keep VS Code imports, folder-specific settings, custom launch files and user keybindings working.

### Pass 4: Explorer and Git at project scale

Q11: selection and bulk file operations first. Then Q09: measure and optimize large change sets, graph navigation,
long paths and refresh stability. Extend the same actions to assistive technology. Include partial staging,
renames, conflicts, dirty branch switches, an unpublished branch and a rejected push in the acceptance matrix.

### Pass 5: release evidence and final polish

Run the repository's strict checks, signed release build and Retina memory budgets. Record actual startup,
frame-time and active-accessibility costs. Test native Windows SSH, IME and mixed DPI on hardware. Track Linux
separately from the shipped macOS/Windows acceptance decision. Finish with one bounded visual polish pass across
English/Turkish, dark/light/high-contrast appearances and narrow editor splits.

## Existing strengths to preserve

- File, terminal, Git and server operations share `Host`; hermetic SSH tests exercise the remote path.
- Semantic theme tokens, built-in contrast checks, reduced-motion settings and Turkish translations already exist.
- Settings preserve JSONC comments; VS Code settings, themes and shortcuts have import paths.
- Rust already has completion, navigation, macro expansion, related diagnostics, LSP configuration, tasks,
  testing and formatter-aware debugging. These are tested capabilities, not missing-feature backlog items.
- Git already has stage/unstage, partial hunks, conflicts, branches, graph, sync, local history and pull requests.
- Architecture, localization, dependency and documentation gates make the first batch reviewable before a main PR.

## Validation log

Initial merge: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo build --locked -p app` passed. The vendored `xattr` dependency emitted existing `uint32_t` deprecation warnings.

Before fixes: `cargo test --locked -p app --bin codeyard product_` ran four new regressions; all four failed for
the observed reasons (empty-folder selection, hidden Explorer, missing Source Control and misleading Rust notice).

The [delivery report](product-quality-report.md) records the native confirmation and validation evidence. After integration,
eight product, 19 Rust hardening, 21 accessibility and 35 container tests passed. All 50 localization checks passed after
removing the integration's two obsolete translation keys and refreshing the template.

The complete local strict run passed 1,698 workspace tests with five documented ignores before the final cancellation
and integration changes; the targeted checks above cover those changes. The initial baseline's ten debugger/SSH timeouts
were not treated as green. Both real LLDB cases also passed independently. The initial product/Rust regressions and both
late-cancellation regressions failed before their fixes. Final-tree cross-platform checks are attached to the delivery PR.

New Turkish strings were drafted by the coding agent; no human native-speaker review is claimed.
