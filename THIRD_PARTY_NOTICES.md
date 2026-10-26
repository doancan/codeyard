# Third-party notices

Codeyard is dual-licensed under MIT or Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). This file lists the material
that is not ours and ships in the repository or in a release build.

## Rust dependencies

Every third-party crate in `Cargo.lock`, with its version and SPDX licence expression, is in `THIRD_PARTY_CRATES.md`
(821 crates; the two vendored ones are marked). Regenerate it with `scripts/crate-licenses.py` after a dependency change;
`scripts/check.sh` fails when it is out of date. It is generated from the `license` field of each crate's `Cargo.toml`
and covers all target platforms. `deny.toml` only allows these licences: MIT, MIT-0, Apache-2.0
(also with the LLVM exception), BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, Unlicense, CC0-1.0, 0BSD, BSL-1.0
and MPL-2.0. `cargo deny check` runs in CI and before each release.

The table lists the licence expression each crate declares; it does not reproduce each crate's own licence file. The
crates' licence files, with their copyright lines, are in the packages on crates.io (and in `vendor/` for the two
vendored crates). Release artifacts ship the vendored and project licence texts (see `docs/releasing.md`), not the
licence file of every crate.

### MPL-2.0 crates

Five crates are MPL-2.0 (file-level copyleft): `nucleo-matcher` 0.3.1 and `spellbook` 0.4.2 (direct dependencies), and
`dwrote` 0.11.5, `option-ext` 0.2.0 and `cbindgen` 0.28.0 (through GPUI; `cbindgen` is a build tool and is not
linked into the app). All are used unmodified, as crates.io dependencies. Their source is available from the repositories below and from crates.io. The licence text is the `LICENSE` file in
each crate package (confirmed for `nucleo-matcher` and `spellbook`; the `license` field of all five is `MPL-2.0`).

- [nucleo-matcher](https://github.com/helix-editor/nucleo), used for the fuzzy matching in the pickers.
- [spellbook](https://github.com/helix-editor/spellbook), the spell-check engine.
- [dwrote](https://github.com/servo/dwrote-rs), [option-ext](https://github.com/soc/option-ext) and
  [cbindgen](https://github.com/mozilla/cbindgen).

## Icons

`crates/app/assets/icons/*.svg` (79 files).

Most are from [Lucide](https://lucide.dev) (ISC licence; the icons derived from Feather are MIT, Copyright (c)
2013-present Cole Bemis). The upstream licence text is in `crates/app/assets/icons/LICENSE`, copied unchanged from
the Lucide repository.

I compared the path data of each file with the Lucide `main` branch:

- 36 files have the same path data as the upstream icon of the same name.
- 30 files have the same name upstream but different path data. They are Lucide icons that were edited, or an older
  version of the icon. I did not check which.
- 6 files have no icon of that name upstream: `alert`, `align-left`, `file-json`, `git-commit`, `history`,
  `sliders`. Lucide has renamed icons of this kind (for example `triangle-alert`, `text-align-start`,
  `file-braces`), so they are probably old or edited Lucide icons. This was not confirmed.
- 3 files, `step-into`, `step-out` and `step-over`, have no upstream counterpart and look like Lucide-style
  drawings made for this project.
- 4 files are not Lucide: `codeyard-mark.svg` (the Codeyard logo) and the filled icons `file-solid.svg`,
  `folder-solid.svg`, `folder-open-solid.svg`. They were drawn for this project and fall under the project licence,
  not under `icons/LICENSE`.

36 + 30 + 6 + 3 + 4 = 79. The icon files use a stroke width of 1.75, not Lucide's default of 2, so every stroked
icon has been modified at least in that attribute.

## Themes

See `crates/theme/themes/README.md`. Palettes inspired by Dracula (MIT, Zeno Rocha), Nord (MIT, Sven Greb /
Arctic Ice Studio), Solarized (MIT, Copyright (c) 2011 Ethan Schoonover), One Dark (MIT, Copyright (c) 2016 GitHub
Inc., Atom) and GitHub Light / Primer (MIT, Copyright (c) 2018 GitHub Inc.). The licence files of Dracula,
Solarized, one-dark-syntax and primer/primitives were read from their GitHub repositories. Nord's licence was
confirmed only through the `license` field of its `package.json`.

Nord needs a caveat. The Nord repository's licence file was not read; only the `license` field of its
`package.json` says MIT.

The Quarry theme (`quarry.json`) and the Codeyard One theme (`codeyard-one.json`, drawn from the colours of the Codeyard icon: graphite and mint) are original palettes written for this project and are not derived from any other theme.

## Spell-check dictionary

`crates/spell/dicts/en/` holds the `en_US` Hunspell dictionary derived from SCOWL (Spell Checker Oriented Word
Lists), version 2020.12.07, http://wordlist.sourceforge.net. Its licence and the credits for the word sources
are in `crates/spell/dicts/en/license`. Keep that file with the dictionary.

## Spell-check engine

[spellbook](https://github.com/helix-editor/spellbook) 0.4.2, MPL-2.0, used unmodified as a dependency. Its licence
is the `LICENSE` file in the crate; source is available from the repository above. See also "MPL-2.0 crates" above.

## Syntax highlighting

[tree-sitter](https://github.com/tree-sitter/tree-sitter) 0.27.1 (MIT) and these grammar crates, all MIT according
to their `Cargo.toml`:

bash, c, c-sharp, cpp, css, dockerfile, go, html, java, javascript, json, kotlin-ng, lua, make, md (markdown), php,
python, ruby, rust, sequel (SQL), swift, toml-ng, typescript, yaml.

The licence text of the runtime, `tree-sitter-language` and every grammar crate is in `crates/syntax/LICENSES/<crate>.txt`
(25 files, each with its own copyright line). Twelve crates ship no LICENSE file in the crates.io package: cpp, html,
java, json, kotlin-ng, lua, make, md, ruby, sequel, toml-ng and typescript. For those the text was taken from the
upstream repository at the release tag (`LICENSE`, or `LICENSE.md` for lua); for md the tag is v0.5.4, the version in
`Cargo.lock`. The other 13 were copied from the unpacked crates.io packages. The `license` field of each of these
crates is `MIT`.

Highlight queries: the grammar crates ship their own queries and the app uses them as they are, except for two
files in `crates/syntax/queries/`:

- `dockerfile.scm` is byte-for-byte identical to `queries/highlights.scm` of tree-sitter-dockerfile 0.2.0 (MIT,
  Copyright (c) 2021 Camden Cheek; compared with the crates.io package). The licence text is the one under
  `crates/syntax/grammars/dockerfile/LICENSE`.
- `kotlin.scm` is original to this project: it was written from the node types of tree-sitter-kotlin-ng, which ships
  no highlight query.

`crates/syntax/grammars/dockerfile/` contains `parser.c` and `scanner.c` from
[tree-sitter-dockerfile](https://github.com/camdencheek/tree-sitter-dockerfile) 0.2.0 (MIT, Copyright (c) 2021 Camden
Cheek), copied unchanged. The licence text is `crates/syntax/grammars/dockerfile/LICENSE`, fetched from the upstream
`v0.2.0` tag; see also `NOTICE` there.

## Vendored code

- `vendor/gpui/`: GPUI 0.2.2 from Zed Industries, Apache-2.0, Copyright 2022 - 2025 Zed Industries, Inc., with local
  patches listed in `vendor/gpui/PATCHES.md`. The licence is `vendor/gpui/LICENSE-APACHE`.
- `vendor/xattr/`: xattr 0.2.3 by Steven Allen, MIT or Apache-2.0, with a one-line Linux patch. Licences are
  `vendor/xattr/LICENSE-MIT` and `vendor/xattr/LICENSE-APACHE`.
