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
| `github-dark.json`, `github-light.json` | GitHub (Primer) | GitHub Inc. / Primer | MIT |
| `catppuccin-mocha.json` | [Catppuccin Mocha](https://github.com/catppuccin/catppuccin) | Catppuccin | MIT |
| `tokyo-night.json` | [Tokyo Night](https://github.com/tokyo-night/tokyo-night-vscode-theme) | Enkia | MIT |
| `gruvbox-dark.json` | [Gruvbox Dark](https://github.com/morhetz/gruvbox) | morhetz | MIT/X11 |

The four dark additions define all 30 roles and pass all 103 contrast pairs as written, before the runtime
contrast guard. Mocha uses mauve accents and pastel syntax; Tokyo Night pairs indigo surfaces with blue accents;
Gruvbox Dark uses warm charcoal, amber and earthy syntax; GitHub Dark uses neutral surfaces and restrained blue accents.
Selection, gutter and comment colours are adjusted where necessary to keep them readable across Codeyard's surfaces.

## Codeyard's own themes

`codeyard-dark.json`, `codeyard-light.json`, `codeyard-one.json` (the graphite tile and mint letter of the app icon), `high-contrast-dark.json`, `high-contrast-light.json`, `studio-dark.json`,
`ember.json`, `fjord.json`, `graphite.json`, `paper.json`, `quarry.json`. The first two follow `crates/theme/design/tokens*.json`.
The others were written for this project. No upstream palette is claimed for them. `quarry.json` is an original palette, designed from scratch and not derived from any existing theme.

Licence texts and the full attribution are in `THIRD_PARTY_NOTICES.md` at the repository root.
