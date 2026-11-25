# File icon sets

The file and folder icons of the icon themes (`"workbench.iconTheme"`: `material`, `catppuccin`, `vscode`, `seti`, `none`) are
real icon files from four open-source projects. They are embedded in the app (`crates/app/src/file_icons_assets.rs`) and
drawn as images, so they keep their own colours; the color theme does not recolor them.

| Theme | Source | Icons | Notes |
| --- | --- | --- | --- |
| Material Icons | Material Icon Theme 5.40.0 | 341 | files by language and tool, 152 folder names with open and closed icons |
| Catppuccin Icons | catppuccin/vscode-icons | 304 per palette | Mocha for dark color themes, Latte for light ones (chosen by the editor background) |
| VS Code Icons | vscode-icons/vscode-icons | 317 | the widest coverage of file names |
| Seti | jesseweed/seti-ui | 95 | single-colour file icons; no folder icons (the tree shows only the arrow) |

Licences: all four are MIT. Their texts are in `crates/app/assets/file-icons/LICENSES/`, are shipped with every release
(`scripts/licenses.sh`) and are listed in `THIRD_PARTY_NOTICES.md`.

## How a name finds its icon

`crates/app/src/icon_theme.rs` reads the set's mapping (`assets/file-icons/<set>.tsv`, tab separated) once:

1. the whole file name, in lower case (`cargo.toml`, `.gitignore`, `dockerfile`);
2. the longest extension that is mapped: every dot but a leading one starts a candidate, longest first (`a.test.ts` tries
   `test.ts`, then `ts`);
3. the set's plain file icon.

A folder is matched by its lower-case name; one that is not known gets the set's plain folder. Open and closed folders have
an icon each.

## Rebuilding the sets

The sets hold the icons the common names need, not everything the projects have: about 170 extensions, 130 file names and
130 folder names (the lists are at the top of `scripts/icon-sets.py`). To change the selection, or to update a set:

```sh
mkdir -p /tmp/icon-sources && cd /tmp/icon-sources
npm pack material-icon-theme@<version> && tar xzf material-icon-theme-*.tgz && mv package material
git clone --depth 1 https://github.com/catppuccin/vscode-icons catppuccin
git clone --depth 1 https://github.com/vscode-icons/vscode-icons vscodeicons
git clone --depth 1 https://github.com/jesseweed/seti-ui seti
cd <repository> && scripts/icon-sets.py --sources /tmp/icon-sources
```

The script replaces `crates/app/assets/file-icons/` and `crates/app/src/file_icons_assets.rs`. The test
`the_mappings_and_the_embedded_icons_agree` fails when a mapping names an icon that is not embedded, or an embedded icon
that nothing maps to. Update the revisions in `THIRD_PARTY_NOTICES.md` and the counts above when you do.
