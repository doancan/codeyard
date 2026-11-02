# Design tokens and themes

Colours are defined once, in `crates/theme/design/tokens.json` (dark) and `tokens.light.json` (light). `crates/theme/
build.rs` turns them into Rust constants at build time (`Palette`, `DARK`, `LIGHT`, `ROLES`, `Palette::get/set`), so
no JSON is parsed at run time and no colour is written by hand in the UI code. Both files must define the same
roles; the build fails when a role is missing.

- **Roles** are semantic (`bg`, `surface`, `surface-raised`, `surface-hover`, `current-line`, `selection`, `border`,
  `text`, `text-muted`, `accent`, `on-accent`, `focus-ring`, `danger`, `warning`, `success`, `info`, `diff-added`,
  `syntax-keyword`, `syntax-string`, …). The UI asks for roles, never for hex values.
- **Built-in themes** (`crates/theme/src/themes.rs`): *Midnight* and *Daylight* come straight from the two token
  files; the others (`codeyard-dark`, `codeyard-light` as the defaults, Nord, Dracula, high-contrast pair, …) are
  JSON files in `crates/theme/themes/` embedded at build time. The defaults follow the system appearance.
- **User themes** are JSON files in `~/.config/codeyard/themes/` that set `"name"`, `"appearance"` and any subset of
  `"color"`; missing roles fall back to the default dark or light palette. Theme files from another editor are mapped
  (workbench colours and TextMate scopes).
- **Contrast.** `Palette::contrast_failures` checks every text role against the backgrounds it is drawn on and is
  asserted for every built-in theme; user themes pass through `guard_contrast`, which nudges a failing foreground
  towards readability. `crates/theme/design/contrast.py` is a standalone WCAG checker for token files.
- **Spacing, radius, type scale and layout sizes** come from the same JSON (`theme::SPACE`, `theme::radius`, `theme::text`,
  `theme::layout`).
- Fonts are not bundled: the system UI and monospace fonts are used; the editor font is a setting.
- Icons are mostly Lucide (ISC, some from Feather under MIT; see `THIRD_PARTY_NOTICES.md`) SVGs in `crates/app/assets/icons/`, embedded in the binary and tinted with the current
  text colour; file-type icons come from `icon_theme.rs`.
