# Workbench UX and engineering follow-up

The review covers Rust/GPUI implementation, extension behaviour and IDE workflows. Independent reviews were cross-checked against regression tests and native runtime behaviour.

## Confirmed issues addressed

| Severity | User-visible issue | Implemented behavior | Verification target |
| --- | --- | --- | --- |
| P1 | Quick Open, command palette and SSH path inputs only appended text; fixing a character in the middle required deleting the suffix. | UTF-8 caret and selection anchors support Left/Right, Home/End, Delete and Shift selection. Partial copy/cut/paste and UTF-16/IME replacement remain consistent. Moving the caret alone does not send remote queries or trigger previews; secret selections cannot be copied. | `picker::tests` |
| P1 | Market Refresh refreshed only installed extensions. A failed registry load could then be treated as fresh for a day. | Refresh follows the selected tab. Explicit Market refresh bypasses only the daily cache shortcut, preserving ETag/signature validation and the current query. Failed/partial loads can retry immediately. Requests are serialized and stale registry-setting responses are discarded. | `market_tests` |
| P2 | Market errors were missing from the accessible status when a partial catalogue existed. | Available results and registry errors are announced together, with an Alert for errors. Healthy results remain usable. | `a11y::extensions_tests` |
| P2 | Terminal find could show “No results” while new matching log output was already highlighted. | The count follows output after a 150 ms quiet period. Work is split into 64-row cancellable turns. Recounts preserve surviving selections, scroll and focus. Resize, scrollback changes and clear operations invalidate old scans. | `terminal::find::live_tests` |
| P2 | Only the first 500 visible terminal matches were tinted; each cell scanned the whole match vector. | All visible matches are retained and a sorted iterator paints them in O(cells + matches). | Dense 2,400-match regression |
| P2 | The fixed terminal find bar overflowed a narrow pane at high UI scale. | The overlay uses pane insets and a maximum width of the available area. | Narrow pane at 200% scale |
| P2 | Settings required contiguous phrases, so “terminal font” could miss the intended setting. | Search matches every word across setting key, title and translated description, in any order. `@modified` combines with words across categories. | `settings_page::tests` |
| P1 | Unsubmitted Settings number/text drafts could disappear when an unrelated setting reloaded. | Track the last committed field text and preserve drafts until acknowledged by the actual settings value. Show a translated Enter-to-apply hint, including in accessible descriptions. Explicit field reset and a successful confirmed reset discard drafts. | Draft preservation, acknowledged save and explicit-reset regressions |
| P2 | Find in Files could not stop without clearing its query. | Stop and Escape cancel work while preserving query, filters and partial results. A second Escape clears the idle query. Shared cancellation invalidates pending structural-buffer generations after workspace changes too. | `search::cancellation_tests` |

## Cross-review corrections

Before runtime verification, peer review caught and corrected stale terminal scan bounds during resize, non-overlapping match counting across a batch boundary (`aaa` in 1,001 `a` characters), and preservation of smart-case matches such as `NEEDLE` for a `needle` query. Central compilation corrected an ambiguous Alacritty `Line` arithmetic expression and a test-context callback signature.

## Coverage boundaries

The shared visual system is retained. This pass does not claim a new complete text editor engine for picker inputs: pointer-based caret positioning and horizontal caret scrolling for very long queries remain inherited limitations. The review did not establish complete IDE feature parity, Windows visual correctness or a screen-reader certification.

Validation results are recorded after the central test, lint and native preview runs below.

## Final validation

- Full application run: **3,316 passed, 0 failed, 6 intentionally ignored** (application tests within `cargo test --workspace`).
- All **14 Settings page tests** passed, including responsive pane bounds, 200% scale, search and draft preservation.
- All **12 live terminal search tests** passed, including separate exact 149/150 ms debounce checks and the 64-row/yield boundary at a tall viewport.
- Final `cargo clippy --workspace --all-targets -- -D warnings`, formatting, architecture, documentation, licence and diff checks passed. The native binary builds successfully.
- The native Settings preview was exercised with the multi-word query `terminal font`; its AX tree exposes the query and result fields. The app keeps the approved light/dark surface system and 40 px header alignment.
- Review screenshot and isolated launcher are under `target/design-review/followup/`. This preview uses local review settings rather than the main user's profile.

The generated translation template intentionally has an empty TAB-delimited column; a file-specific `.gitattributes` whitespace rule preserves that format without flagging its necessary trailing delimiter.

The draft-preservation preview also exercised an unsubmitted terminal-font draft (`18`) while the committed setting remained `13`. An unrelated settings-file change was applied; a fresh AX tree still reported the field value `18`, confirming runtime draft preservation. The later layout preview shows the spacious Turkish form at the normal committed values.


## Settings layout and lifecycle verification

The Settings page now uses a single readable form column. Labels, descriptions and configuration keys sit above controls; text fields expand up to 480 px and numeric fields to 180 px, both constrained by the available pane. Controls, errors, resets, draft hints and source notes retain their existing actions and accessibility nodes.

A separate search row stays clear of utility actions. The title and utilities share a toolbar; compact panes use labelled icon tooltips. A measured 760 px pane breakpoint switches the 208 px category rail to a horizontally scrollable strip, independently of window width. The compact toolbar preserves content space with workbench chrome at 200% scale. Rendered-bounds tests cover 480/800/1280 px windows at 100% and 200%, plus the complete 1280×800 workbench at 200%.

Native screenshots cover wide and compact light/dark forms, Turkish interface text, search results and 200% scaling. The isolated preview remains under `target/design-review/followup/`.

A lifecycle regression fixture now completes an accepted detached-window close by removing the test window. The test platform's `simulate_close` only invokes the close handler; leaving both windows rendering one terminal caused repeated grid resizing and an output-feedback loop. The test now verifies that only the main window remains before asserting the workbench is released. Production close handling is unchanged.


The final `scripts/check.sh --strict --budgets` run passed: full workspace tests, formatting, strict Clippy, architecture/documentation/licence checks, dependency policies and vulnerability audit. Turkish translation coverage is 4,162/4,162 strings. The release verifier self-test passed all 23 cases.

Retina memory measurements stayed within the existing budgets: terminal 81 MB, welcome 72 MB, Rust file 108 MB, 10k-line plain text 93 MB, highlighted 10k-line file 117 MB, first-run question 78 MB and idle remote agent 1 MB. The application binary is 76 MB.

The interleaved comparison against v0.12.8 passed every blocking performance metric (five timing runs and three memory samples per scene). Median memory was 72 MB for the welcome screen, 117 MB for a highlighted 10k-line file and 80 MB for the terminal. Cold extension activation remains about 352 ms in both builds, above the advisory 150 ms target. Raw samples and the complete comparison are in `docs/benchmarks/perf-0.12.9.md`.
