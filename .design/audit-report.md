# Codeyard theme and workbench audit

Scope: the native GPUI workbench on macOS, all built-in themes, interface/file icons, editor chrome and terminal rendering. This is a desktop audit; mobile touch-target rules do not apply. Product workflows, native window controls and the existing accessibility implementation are retained.

## Confirmed findings

| Severity | Finding and evidence | Resolution |
| --- | --- | --- |
| P1 | Terminal `drain_events` discarded Alacritty `ColorRequest` events. A local PTY probe of an installed terminal program emitted OSC 10 and OSC 11 queries. With no response, a program cannot discover the actual light background. | Reply to OSC 4/10/11/12 using the same palette and program overrides as rendering. A real terminal probe received `rgb:2020/2626/3030` and `rgb:fdfd/fdfd/fefe` in Codeyard Light. |
| P1 | Terminal faint text was blended 40% toward the application background without checking its final cell background. True-colour/indexed output bypassed palette contrast checks. Examples from the reported light terminal: `#cba6f7`/white = 2.03:1; `#a6e3a1`/white = 1.49:1. | Apply a 4.5:1 text floor after inverse video and selection/search background resolution. Preserve background colours, concealed text and intentional equal-colour pairs. |
| P2 | Explorer used `VIEW_HEADER_H + 6` with top padding while editor tabs used `TABBAR_H`. This created a visible offset between adjoining rules. Other panel headers used a third height. | One shared `pane_header` primitive, with 40 px primary header geometry, centred content and a full-width separator. A rendered-bounds regression check compares Explorer, editor tabs and the panel toolbar. |
| P2 | The activity rail's removed divider left interrupted visual boundaries. Dark-theme separators remained visually strong. | Restore the full-height rail divider and soften decorative border tokens across all regular themes. Keep high-contrast dividers, control borders and keyboard focus distinct. |
| P2 | Interface icons mixed multiple generations and custom glyph geometries. | Replace 112 glyphs with a consistent Tabler family; normalize outlines to 1.65 px on a 24 px grid. Preserve the project logo. Ship pinned upstream attribution and licence in macOS/Windows release packaging. |
| P2 | Increasing title/header space reduced the activity rail to 46 px at 480×320 and 200% scale. | A compact 24 px status bar preserves a complete activity target; the English and Turkish narrow-window tests pass. |

## Palette work

All previous theme IDs remain stable. All 31 built-in palettes are checked as authored, rather than allowing a runtime repair to mask a defective built-in. The existing palette files already passed their documented static pairs before this work; the reported terminal failure was outside that coverage.

Surfaces, hover/selected fills, comments, text, borders and semantic colours were balanced together. Published palette variants use balanced neutral surfaces and separator roles adapted for an editor. Eight original palettes were added: Basalt, Porcelain, Canopy, Meadow, Deepwater, Shore, Mulberry and Vellum. These palettes use the application's shared semantic colour roles.

## Rust implementation

- Reuse semantic tokens and a shared primary header primitive instead of repeating independent geometry.
- Cache terminal contrast corrections by foreground/background/faint state within each frame. Apply the same result to spaces so colour correction does not fragment shaped text at every word.
- Match terminal events by ownership, avoiding cloning every PTY response and its string.
- Bound protocol colour lookup to supported indices; honour application-set colour overrides.
- Keep contrast logic separate from VT parsing, PTY lifecycle and UI layout.
- Add regression coverage for real parser-to-PTY colour replies and ANSI, indexed, true-colour and faint text across all themes.

## Verification and limits

- 31 palettes × 103 raw pairs = 3,193 contrast checks passed, including the two token palettes.
- Terminal protocol and readability tests cover all 31 themes and program background overrides.
- Initial complete application test pass exposed stale theme/font documentation, the narrow-window regression and an old hard-coded status-bar height assertion; these were corrected.
- Native visual review covers light/dark editor and terminal, icon consistency and pane boundaries. The local review gallery uses screenshots of the native app, not HTML recreations.
- macOS is the exercised desktop platform. This work does not claim a VoiceOver certification, a Windows visual pass or a whole-product accessibility certification.
- Build output retains the existing macOS linker compact-unwind size warning and future-incompatibility notices from dependencies; these do not prevent the build.

Sources: [Primer colour usage](https://primer.style/product/getting-started/foundations/color-usage/), [Tabler Icons](https://github.com/tabler/tabler-icons) at `85e870dbe8680c2d6deefd062122d30ef1544a41`.

Final application validation for 0.12.9: `cargo test --workspace` passed the application's 3,316 tests, with 6 intentionally ignored. The 31-palette contrast coverage and 17 theme tests remain in the release gate. Native light/dark and Turkish Settings previews exercise the real application; the full workspace, memory and comparative performance results are recorded in the follow-up report.

Live accessibility-tree inspection on the macOS preview confirmed labelled view controls, editor tabs, bottom-panel tabs and the terminal text area. This was an AX tree inspection, not a screen-reader session. Interface SVG validation parsed all 113 assets (112 replaced glyphs plus the project logo).

Review artifacts: `target/design-review/index.html` compares 31 native theme captures. `target/design-review/launch.sh` reopens the isolated native preview; `target/design-review/config` and `state` contain its local preferences and session. The active preview uses the final compiled binary.
