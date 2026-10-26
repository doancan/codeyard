# Built-in themes

Each file is one theme. The keys are Codeyard's colour roles (see `docs/design-tokens.md`), not the keys of any
other editor. A theme is checked by the contrast test in the theme crate.

## Themes based on a published palette

These use the colours of a well-known palette, mapped onto Codeyard's roles. Where a role has no counterpart in the
original (for example `surface-raised`, `diff-added`), the value was chosen by hand. Do not read these files as an
exact copy of the upstream theme.

| File | Palette | Author | Licence of the upstream project |
| --- | --- | --- | --- |
| `dracula.json` | Dracula | Zeno Rocha and contributors | MIT |
| `nord.json` | Nord | Sven Greb (Arctic Ice Studio) | MIT |
| `solarized-dark.json`, `solarized-light.json` | Solarized | Ethan Schoonover | MIT |
| `one-dark.json` | One Dark (Atom) | GitHub Inc. | MIT |
| `github-light.json` | GitHub Light (Primer) | GitHub Inc. | MIT |

## Codeyard's own themes

`codeyard-dark.json`, `codeyard-light.json`, `codeyard-one.json` (the graphite tile and mint letter of the app icon), `high-contrast-dark.json`, `high-contrast-light.json`, `studio-dark.json`,
`ember.json`, `fjord.json`, `graphite.json`, `paper.json`, `quarry.json`. The first two follow `crates/theme/design/tokens*.json`.
The others were written for this project. No upstream palette is claimed for them. `quarry.json` is an original palette, designed from scratch and not derived from any existing theme.

Licence texts and the full attribution are in `THIRD_PARTY_NOTICES.md` at the repository root.
