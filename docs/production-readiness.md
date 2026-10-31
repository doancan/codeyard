# Production readiness review

Review date: 2026-10-27. Scope: the native editor, local and SSH file operations, trust gates,
preview resources, settings/state, maintainability, documentation and release checks. This review
does not add a new product feature.

## Baseline and findings

`develop` was synchronized with `main` at `eabbd82`. Its Markdown reading view (`360f49c`)
was retained. Both sides of the translation-table conflicts were preserved.
The concurrent container-workspace and Windows-title-bar changes through `db1badb` were then
merged from remote `develop`, together with the heading-anchor collision fix. They were preserved
and included in the final validation rather than overwritten.

| Area | Evidence and assessment | Action | Status |
| --- | --- | --- | --- |
| Image safety | `large.rs` checks decoded dimensions; Markdown reads images without that check. A tiny 30,000 × 30,000 PNG can bypass the normal preview guard. | Share image loading and validation; cover both entry points. | Done in #15 |
| SVG sizing | The old attribute scan mistakes a root in a comment for the real root and misses XML spacing and physical units. | Use GPUI's existing `usvg` dependency for canvas dimensions, with linked-resource loading disabled during validation. | Done in #15 |
| Resource lifecycle | Markdown retains images by URL and finishes loads asynchronously. A removed and re-added URL can receive an obsolete result; cache membership checks are linear. | Track load identity and use a set of current resources. | Done in #15 |
| Trust | `git/status.rs` disables fsmonitor and pauses untrusted repositories with program settings; task/debug/test entry points gate execution. | Preserve these checks and run their real-process regressions. | Ongoing gate |
| File integrity | Agent reads are capped at `proto::MAX_FILE`, including growth during reads; writes use conflict hashes; profile writes use atomic replacement. | Exercise workspace and SSH suites. | Ongoing gate |
| Updates | Signed manifests, per-asset hashes and replay tracking are implemented; mocked signing/notarization tests exist. | Run verification and credential-cleanup tests. | Ongoing gate |
| Release tooling | Both signing scripts reject OpenSSL 4 solely because its version does not start with 3. | Check for `pkeyutl -rawin` support; retain actual Ed25519 signing and verification. | Done in #15 |
| Dependency coverage | GitHub flags `grid 0.18.0` through GPUI/Taffy for GHSA-38c5-483c-4qqp, while the RustSec scan passes. | Update Taffy to compatible 0.9.2, resolve fixed grid and ban affected versions in cargo-deny. | Done in #15 |
| Benchmark isolation | Normal memory scenes also open the first-run update question, including its extra native-window buffers. Main measures 89 MB for terminal and 99 MB for plain text; develop measures 78/89 MB after choosing manual updates. | Isolate normal editor measurements; add a separate first-run question scene without raising existing budgets. | Done in #15 |
| Test isolation | Generic editor/workbench fixtures enable LSP and can launch installed servers while testing unrelated editing/debugging behavior. | Turn LSP off in generic fixtures; explicitly enable it in LSP integration tests. | Done in #207 |
| Maintainability | Topic files and state groups are enforced by `arch-check.sh`; theme tokens and settings catalogs are sources of truth. | Keep limits unchanged; centralize preview policy. | Ongoing gate |
| Documentation | Contributing/platform/architecture documents still describe experimental Linux CI and unverified notarization, although CI and release evidence have advanced. | Align statements with current workflows and recorded evidence. | Done in #15 |
| Interface | Markdown has layout/scroll regressions and safe link handling; welcome colors have contrast tests. | Check real rendering with isolated state and run both language/theme regressions. | Ongoing gate |
| Painted SVGs | Real-window inspection found red/blue channel reversal and darkened transparency in SVG image previews. | Convert premultiplied RGBA to straight BGRA in GPUI's image path; compare opaque and translucent pixels in a regression. | Done in #15 |
| Paragraph layout | A long paragraph measured 1,548 px inside an 820 px document and visibly overflowed. | Constrain paragraph children with a column layout; test the shaped text bounds. | Done in #15 |
| Text link gestures | A real link regression showed a right-click opening the linked file. | Accept only left clicks, register press/release in one frame and clear cancellation; cover right clicks, outside releases and a same-frame click. | Done in #15 |
| Resize feedback | The requested hover feedback switches abruptly to a full accent stripe. | Share a 130 ms color fade in and 100 ms fade out across panel, editor and section handles; retain their hitboxes and full feedback during dragging, with no spatial motion or idle animation loop. | Done in #15 |
| Concurrent integration | The incoming CI failed strict Clippy because two container modules put implementation after tests; an SSH assertion expected a container group that the new default disables, and a split-terminal test ignored a possible startup-process close prompt. | Move test modules to the end, check the disabled module's actual contract and handle the normal confirmation in the split regression. | Done in #15 |

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
  were removed. Both subsequent complete local runs passed 1,516 tests, with zero failures and three
  deliberate manual/data-dependent ignores. The second run includes Taffy 0.9.2 / grid 1.0.1:
  `CODEYARD_REQUIRE_SSHD=1 CODEYARD_TEST_TIMEOUT_SCALE=3 cargo test --locked --workspace`.
- `scripts/memcheck.sh` passed with isolated editor scenes on the local Retina Mac. Measurements:

| Scene | Measured | Budget |
| --- | ---: | ---: |
| Terminal | 78 MB | 85 MB |
| Welcome | 78 MB | 85 MB |
| Rust file | 96 MB | 125 MB |
| 10k-line plain text | 89 MB | 95 MB |
| 10k-line highlighted file | 119 MB | 130 MB |
| First-run update question | 85 MB | 95 MB |
| Idle remote agent | 2 MB | 8 MB |
| App binary | 59 MB | 60 MB |
| x86_64 Linux musl agent | 1,568 KB | 1,600 KB |
| aarch64 Linux musl agent | 1,360 KB | 1,600 KB |

Both Linux agents were rebuilt with cargo-zigbuild using the release-agent profile; a stale x64
artifact from a different build had initially exceeded its size limit.

After Computer Use access was enabled, the isolated release bundle was inspected in a real native
window. Dark-theme SVG colors and paragraph/list/table layout were checked at normal and maximized
window sizes; a single left click opened the relative document. These checks exposed the SVG channel
and paragraph-width bugs above. Their fixes passed a full strict run: 1,519 tests, zero failures and
three deliberate ignores. Regression tests also verify same-frame link clicks, right-click rejection
and cancellation outside the link. This does not establish screen-reader results. Automated Markdown
checks cover wheel events, live source updates and safe resource errors.

[PR #15](https://github.com/doancan/codeyard/pull/15) carries the platform check results. Required gates are
macOS tests and budgets, Windows native tests and standalone packaging, Linux libraries/agent and the Linux
app workspace, plus dependency/license gates. Optional coverage is excluded from the merge requirement.
