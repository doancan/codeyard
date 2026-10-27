# Production readiness review

Review date: 2026-10-27. Scope: the native editor, local and SSH file operations, trust gates,
preview resources, settings/state, maintainability, documentation and release checks. This review
does not add a new product feature.

## Baseline and findings

`develop` was synchronized with `main` at `eabbd82`. Its Markdown reading view (`360f49c`)
was retained. Both sides of the translation-table conflicts were preserved.

| Area | Evidence and assessment | Action |
| --- | --- | --- |
| Image safety | `large.rs` checks decoded dimensions; Markdown reads images without that check. A tiny 30,000 × 30,000 PNG can bypass the normal preview guard. | Share image loading and validation; cover both entry points. |
| SVG sizing | The old attribute scan mistakes a root in a comment for the real root and misses XML spacing and physical units. | Use GPUI's existing `usvg` dependency for canvas dimensions, with linked-resource loading disabled during validation. |
| Resource lifecycle | Markdown retains images by URL and finishes loads asynchronously. A removed and re-added URL can receive an obsolete result; cache membership checks are linear. | Track load identity and use a set of current resources. |
| Trust | `git/status.rs` disables fsmonitor and pauses untrusted repositories with program settings; task/debug/test entry points gate execution. | Preserve these checks and run their real-process regressions. |
| File integrity | Agent reads are capped at `proto::MAX_FILE`, including growth during reads; writes use conflict hashes; profile writes use atomic replacement. | Exercise workspace and SSH suites. |
| Updates | Signed manifests, per-asset hashes and replay tracking are implemented; mocked signing/notarization tests exist. | Run verification and credential-cleanup tests. |
| Release tooling | Both signing scripts reject OpenSSL 4 solely because its version does not start with 3. | Check for `pkeyutl -rawin` support; retain actual Ed25519 signing and verification. |
| Test isolation | Generic editor/workbench fixtures enable LSP and can launch installed servers while testing unrelated editing/debugging behavior. | Turn LSP off in generic fixtures; explicitly enable it in LSP integration tests. |
| Maintainability | Topic files and state groups are enforced by `arch-check.sh`; theme tokens and settings catalogs are sources of truth. | Keep limits unchanged; centralize preview policy. |
| Documentation | Contributing/platform/architecture documents still describe experimental Linux CI and unverified notarization, although CI and release evidence have advanced. | Align statements with current workflows and recorded evidence. |
| Interface | Markdown has layout/scroll regressions and safe link handling; welcome colors have contrast tests. | Check real rendering with isolated state and run both language/theme regressions. |

## Implementation and validation plan

1. Reproduce the Markdown image safety gap with a regression test before fixing it.
2. Consolidate image checks and loading, then harden asynchronous resource ownership.
3. Correct conflicting operational documentation and record user-visible changes.
4. Run formatting, strict Clippy, architecture/documentation/license checks, dependency advisories,
   the full workspace suite with required sshd, release verification and notarization tests.
5. Measure release memory/binary budgets on the local Mac and inspect the real window using a
   throw-away profile. Push `develop`, open the PR to `main`, and require the macOS, Windows
   and Linux jobs to pass before merging.

## Release scope and verification limits

The supported release targets remain Apple-silicon macOS and Windows x64. Linux has blocking
workspace CI but no released installer. Physical Windows SSH authentication, mixed DPI and IME,
and assistive-technology journeys remain unverified as described in [platforms](platforms.md)
and [accessibility](accessibility.md). Green automated checks do not establish those results.

## Validation evidence

- The new Markdown image regression failed before the fix: `huge.png` was accepted as `Ok(())`.
  Afterwards all 10 image/resource tests passed, including standalone tabs, SVG XML/units,
  encoded-size rejection and stale completion ownership.
- `scripts/check.sh --quick --strict` passed: format, Clippy with warnings denied, architecture,
  documents, licenses, cargo-deny and cargo-audit. The existing advisory exceptions were unchanged.
- Release verification initially failed on OpenSSL 4.0.3. With capability detection, all 23
  signature/checksum/tampering scenarios passed. All six notarization/credential-cleanup tests passed.
- The first full workspace run found the translation-template merge ordering error and 13
  performance/process timing failures while other builds were running. The template was regenerated;
  the scripted debugger scenario passed independently in 1.28 seconds. Generic fixture LSP launches
  were removed. Final full-suite and platform results will be recorded before merge.
