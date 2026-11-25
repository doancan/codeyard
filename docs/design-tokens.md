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
- **Contrast.** `Palette::contrast_failures` checks every text role against the backgrounds it is drawn on.
  All 31 built-in themes must pass as authored, before `guard_contrast` runs. Imported themes are repaired;
  native user themes and user overrides are kept as written, with a contrast notice for failing pairs.
  `crates/theme/design/contrast.py` is a standalone WCAG checker for token files.
- **Terminal.** OSC 4/10/11/12 queries return the current palette and program overrides, so terminal programs can
  discover light and dark backgrounds. Text has a 4.5:1 contrast floor against its final background, after inverse
  video, selection and search highlights. Faint text keeps a hierarchy without dropping below that floor.
  Colour corrections are cached per foreground/background/faint combination for the current frame; backgrounds,
  hidden text and deliberate equal-colour pairs are preserved.
- **Spacing, radius, type scale and layout sizes** come from the same JSON (`theme::SPACE`, `theme::radius`, `theme::text`,
  `theme::layout`).
- Fonts are not bundled: the system UI and monospace fonts are used; the editor font is a setting.
- The 105 interface icons use Tabler (MIT; see `THIRD_PARTY_NOTICES.md`), with a consistent 24 px grid
  and 1.65 px outline stroke. SVGs in `crates/app/assets/icons/` are embedded and tinted with the current theme;
  file-type icons come from `icon_theme.rs`. The Codeyard logo is authored by the project.
