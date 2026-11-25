#!/usr/bin/env python3
"""Builds the file icon sets Codeyard embeds from the upstream projects' sources (see docs/file-icons.md).

    scripts/icon-sets.py --sources DIR [--out crates/app]

`--sources` holds four checkouts: `material` (the npm package material-icon-theme), `catppuccin` (catppuccin/vscode-icons),
`vscodeicons` (vscode-icons/vscode-icons) and `seti` (jesseweed/seti-ui). For every set it keeps the icons that the common file
names, extensions and folder names resolve to (the lists below), copies them to `assets/file-icons/<set>/` and writes the
mapping `assets/file-icons/<set>.tsv`, and writes `src/file_icons_assets.rs`, the table that embeds them.

Mapping lines are tab separated: `n name icon` (a whole file name), `e extension icon` (without the dot; may hold dots),
`f folder icon open-icon`, `d file icon` and `d folder icon open-icon` (the defaults). Names are lower case.
"""

import argparse
import json
import re
import shutil
import sys
from pathlib import Path

EXTENSIONS = """rs ts tsx mts cts js jsx mjs cjs py pyi pyw go c h cpp cc cxx hpp hh cs java kt kts swift m mm rb php lua zig dart scala
clj cljs ex exs erl hs ml mli r jl pl pm sh bash zsh fish ps1 psm1 bat cmd html htm xhtml css scss sass less styl vue svelte astro
json jsonc json5 jsonl toml yaml yml xml xsd xsl ini conf cfg env properties md mdx markdown txt rst adoc tex bib csv tsv sql db
sqlite sqlite3 graphql gql proto thrift lock log diff patch svg png jpg jpeg gif webp ico bmp tiff avif heic psd ai sketch fig
mp3 wav ogg flac m4a aac mp4 mov avi mkv webm m4v pdf doc docx odt xls xlsx ods ppt pptx odp key numbers pages zip tar gz tgz xz
bz2 zst 7z rar dmg iso img exe msi dll so dylib wasm wat o a obj class jar war lib ttf otf woff woff2 eot ipynb tf tfvars hcl nix
gradle cmake mk make vim el lisp scm rkt sol nim cr v odin gleam pem crt cer key asc gpg pub http rest prisma mod sum hbs ejs pug
jinja njk twig liquid erb rake gemspec podspec plist xcconfig entitlements storyboard xib pbxproj xcworkspace xcodeproj apk aab
ipa d.ts test.ts spec.ts test.tsx spec.tsx test.js spec.js test.jsx spec.jsx stories.tsx stories.ts config.js config.ts
module.css module.scss min.js min.css map tsbuildinfo cjs.map bicep proto lockb dockerfile gitignore editorconfig""".split()

NAMES = """package.json package-lock.json yarn.lock pnpm-lock.yaml bun.lockb cargo.toml cargo.lock go.mod go.sum pyproject.toml
requirements.txt pipfile pipfile.lock poetry.lock setup.py setup.cfg tox.ini gemfile gemfile.lock rakefile composer.json
composer.lock pom.xml build.gradle build.gradle.kts settings.gradle settings.gradle.kts gradlew gradle.properties cmakelists.txt
makefile justfile dockerfile docker-compose.yml docker-compose.yaml compose.yml compose.yaml .dockerignore .gitignore
.gitattributes .gitmodules .gitkeep .editorconfig .prettierrc .prettierrc.json .prettierignore .eslintrc .eslintrc.js
.eslintrc.json .eslintignore eslint.config.js eslint.config.mjs eslint.config.ts .babelrc babel.config.js tsconfig.json
tsconfig.base.json jsconfig.json vite.config.ts vite.config.js webpack.config.js rollup.config.js next.config.js next.config.mjs
next.config.ts nuxt.config.ts svelte.config.js astro.config.mjs tailwind.config.js tailwind.config.ts postcss.config.js
postcss.config.cjs jest.config.js jest.config.ts vitest.config.ts playwright.config.ts cypress.config.ts .env .env.local
.env.example .env.development .env.production readme.md readme readme.txt license license.md license.txt licence copying
notice changelog.md changelog contributing.md code_of_conduct.md security.md authors codeowners .nvmrc .node-version
.tool-versions .python-version .ruby-version rust-toolchain rust-toolchain.toml rustfmt.toml .rustfmt.toml clippy.toml deny.toml
jenkinsfile .travis.yml .gitlab-ci.yml azure-pipelines.yml vercel.json netlify.toml firebase.json renovate.json .npmrc .yarnrc
.yarnrc.yml .browserslistrc .stylelintrc procfile vagrantfile nginx.conf robots.txt favicon.ico .mailmap cname
.swiftlint.yml package.swift podfile cartfile mod.rs lib.rs main.rs build.rs index.ts index.js index.html main.ts main.go
main.py __init__.py manage.py app.py""".split()

FOLDERS = """src source sources lib libs bin build dist out target node_modules vendor .git .github .gitlab .vscode .idea test tests
spec specs __tests__ __mocks__ e2e docs doc documentation examples example samples scripts script tools util utils helpers
helper config configs .config public static assets images img icons fonts media styles css scss components component views
view pages routes layouts templates models model controllers services middleware api app apps packages package core common
shared types interfaces hooks store stores state context contexts plugins extensions modules migrations db database data
fixtures mocks stubs logs log tmp temp cache .cache coverage .next .nuxt .svelte-kit .turbo docker k8s kubernetes terraform
ansible ci .circleci workflows android ios macos windows linux web mobile desktop server client frontend backend cmd internal
pkg crates sdk deploy installer perf actions .husky .storybook locales i18n lang translations themes theme animations audio
video sounds resources res schemas proto generated gen include inc obj debug release dev prod staging .devcontainer
.vscode-test benchmarks bench examples demo demos site www""".split()

SETI_COLORS = {}


def read(path):
    return Path(path).read_text(encoding="utf-8")


def copy_icon(src, dst_dir, name, used):
    """Copies `src` to `dst_dir/name.svg` once."""
    if name in used:
        return True
    if not src.is_file():
        return False
    dst_dir.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, dst_dir / f"{name}.svg")
    used.add(name)
    return True


class Set:
    def __init__(self, name):
        self.name = name
        self.names = {}
        self.exts = {}
        self.folders = {}
        self.defaults = {}
        self.used = set()

    def tsv(self):
        lines = []
        for k, v in sorted(self.names.items()):
            lines.append(f"n\t{k}\t{v}")
        for k, v in sorted(self.exts.items()):
            lines.append(f"e\t{k}\t{v}")
        for k, (c, o) in sorted(self.folders.items()):
            lines.append(f"f\t{k}\t{c}\t{o}")
        for k, v in sorted(self.defaults.items()):
            lines.append("d\t" + k + "\t" + (v if isinstance(v, str) else "\t".join(v)))
        return "\n".join(lines) + "\n"


# --- Material Icon Theme ---------------------------------------------------------------------------------------------

def build_material(src, out):
    s = Set("material")
    root = Path(src) / "material"
    m = json.loads(read(root / "dist/material-icons.json"))
    defs = m["iconDefinitions"]
    dst = out / "assets/file-icons/material"

    def icon(key):
        path = defs.get(key, {}).get("iconPath")
        if not path:
            return None
        stem = Path(path).stem
        return stem if copy_icon(root / "icons" / f"{stem}.svg", dst, stem, s.used) else None

    for n in NAMES:
        if (k := m["fileNames"].get(n)) and (i := icon(k)):
            s.names[n] = i
    for e in EXTENSIONS:
        if (k := m["fileExtensions"].get(e)) and (i := icon(k)):
            s.exts[e] = i
    for f in FOLDERS:
        k = m["folderNames"].get(f)
        o = m["folderNamesExpanded"].get(f)
        if k and o and (c := icon(k)) and (op := icon(o)):
            s.folders[f] = (c, op)
    s.defaults["file"] = icon(m["file"])
    s.defaults["folder"] = (icon(m["folder"]), icon(m["folderExpanded"]))
    return s


# --- Catppuccin ------------------------------------------------------------------------------------------------------

def ts_lists(block):
    out = {}
    for key in ("languageIds", "fileExtensions", "fileNames"):
        mt = re.search(key + r":\s*\[(.*?)\]", block, re.S)
        out[key] = re.findall(r"'((?:[^'\\]|\\.)*)'", mt.group(1)) if mt else []
    return out


def ts_records(text, indent="  "):
    """`'key': { ... }` records at one indentation level of a TypeScript object literal."""
    pattern = re.compile(r"^" + indent + r"'([^']+)':\s*\{(.*?)^" + indent + r"\},?", re.S | re.M)
    return [(m.group(1), m.group(2)) for m in pattern.finditer(text)]


def build_catppuccin(src, out, flavor):
    s = Set(f"catppuccin-{flavor}")
    root = Path(src) / "catppuccin"
    icons = root / "icons" / flavor
    dst = out / f"assets/file-icons/catppuccin-{flavor}"
    files = {k: ts_lists(b) for k, b in ts_records(read(root / "src/defaults/fileIcons.ts"))}
    folders = {k: re.findall(r"'((?:[^'\\]|\\.)*)'", re.search(r"folderNames:\s*\[(.*?)\]", b, re.S).group(1))
               for k, b in ts_records(read(root / "src/defaults/folderIcons.ts")) if "folderNames" in b}
    by_ext, by_name = {}, {}
    for icon, lists in files.items():
        for e in lists["fileExtensions"]:
            by_ext.setdefault(e.lower(), icon)
        for n in lists["fileNames"]:
            by_name.setdefault(n.lower(), icon)
    by_folder = {}
    for icon, names in folders.items():
        for n in names:
            by_folder.setdefault(n.lower(), icon)

    def take(stem):
        return stem if copy_icon(icons / f"{stem}.svg", dst, stem, s.used) else None

    for n in NAMES:
        if (k := by_name.get(n)) and (i := take(k)):
            s.names[n] = i
    for e in EXTENSIONS:
        if (k := by_ext.get(e)) and (i := take(k)):
            s.exts[e] = i
    for f in FOLDERS:
        if (k := by_folder.get(f)):
            c, o = take(f"folder_{k}"), take(f"folder_{k}_open")
            if c and o:
                s.folders[f] = (c, o)
    s.defaults["file"] = take("_file")
    s.defaults["folder"] = (take("_folder"), take("_folder_open"))
    return s


# --- vscode-icons ----------------------------------------------------------------------------------------------------

def vsc_entries(text):
    """The `supported: [ { icon, extensions, ... }, ... ]` entries of a manifest source file."""
    start = text.index("supported: [") + len("supported: [")
    entries, depth, begin = [], 0, None
    for i in range(start, len(text)):
        ch = text[i]
        if ch == "{":
            if depth == 0:
                begin = i
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0 and begin is not None:
                entries.append(text[begin:i + 1])
                begin = None
        elif ch == "]" and depth == 0:
            break
    return entries


def build_vscode(src, out):
    s = Set("vscode")
    root = Path(src) / "vscodeicons"
    icons = root / "icons"
    dst = out / "assets/file-icons/vscode"
    by_ext, by_name, by_folder = {}, {}, {}
    langs = {}
    for m in re.finditer(r"^  (\w+): \{(.*?)\},?$", read(root / "src/iconsManifest/languages.ts"), re.S | re.M):
        known = re.search(r"knownExtensions:\s*\[(.*?)\]", m.group(2), re.S)
        langs[m.group(1)] = re.findall(r"'((?:[^'\\]|\\.)*)'", known.group(1)) if known else []
    for entry in vsc_entries(read(root / "src/iconsManifest/supportedExtensions.ts")):
        if re.search(r"disabled:\s*true", entry):
            continue
        icon = re.search(r"icon:\s*'([^']+)'", entry).group(1)
        mt = re.search(r"extensions:\s*\[(.*?)\]", entry, re.S)
        keys = [k.lower() for k in re.findall(r"'((?:[^'\\]|\\.)*)'", mt.group(1))] if mt else []
        ml = re.search(r"languages:\s*\[(.*?)\]", entry, re.S)
        for lang in re.findall(r"languages\.(\w+)", ml.group(1)) if ml else []:
            keys += [k.lower() for k in langs.get(lang, [])]
        if not keys:
            continue
        target = by_name if re.search(r"filename:\s*true", entry) else by_ext
        for k in keys:
            target.setdefault(k.lstrip(".") if target is by_ext else k, icon)
    for entry in vsc_entries(read(root / "src/iconsManifest/supportedFolders.ts")):
        if re.search(r"disabled:\s*true", entry):
            continue
        icon = re.search(r"icon:\s*'([^']+)'", entry).group(1)
        mt = re.search(r"extensions:\s*\[(.*?)\]", entry, re.S)
        for k in re.findall(r"'((?:[^'\\]|\\.)*)'", mt.group(1)) if mt else []:
            by_folder.setdefault(k.lower(), icon)

    def take(kind, icon, suffix=""):
        stem = f"{kind}_{icon}{suffix}"
        return stem if copy_icon(icons / f"{stem}.svg", dst, stem, s.used) else None

    for n in NAMES:
        if (k := by_name.get(n)) and (i := take("file_type", k)):
            s.names[n] = i
    for e in EXTENSIONS:
        if (k := by_ext.get(e)) and (i := take("file_type", k)):
            s.exts[e] = i
    for f in FOLDERS:
        k = by_folder.get(f) or by_folder.get(f.lstrip("."))
        if k:
            c, o = take("folder_type", k), take("folder_type", k, "_opened")
            if c and o:
                s.folders[f] = (c, o)
    s.defaults["file"] = take("default", "file") or copy_icon(icons / "default_file.svg", dst, "default_file", s.used) and "default_file"
    s.defaults["folder"] = (
        "default_folder" if copy_icon(icons / "default_folder.svg", dst, "default_folder", s.used) else None,
        "default_folder_opened" if copy_icon(icons / "default_folder_opened.svg", dst, "default_folder_opened", s.used) else None,
    )
    return s


# --- Seti UI ---------------------------------------------------------------------------------------------------------

def seti_colors(root):
    text = read(root / "styles/ui-variables.less") + read(root / "styles/components/icons.less") if False else ""
    colors = {}
    for f in (root / "styles").rglob("*.less"):
        for m in re.finditer(r"^@([\w-]+):\s*(#[0-9a-fA-F]{3,6})\s*;", read(f), re.M):
            colors.setdefault(m.group(1), m.group(2))
    return colors


def build_seti(src, out):
    s = Set("seti")
    root = Path(src) / "seti"
    icons = root / "icons"
    dst = out / "assets/file-icons/seti"
    colors = seti_colors(root)
    mapping = {}
    for f in (root / "styles/components/icons").glob("*.less"):
        for m in re.finditer(r'\.icon-set\("([^"]+)",\s*"([^"]+)",\s*@([\w-]+)\)', read(f)):
            mapping.setdefault(m.group(1).lower(), (m.group(2), m.group(3)))

    def take(icon, color):
        hexcolor = colors.get(color)
        stem = f"{icon}-{color}"
        if stem in s.used or not hexcolor:
            return stem if stem in s.used else None
        path = icons / f"{icon}.svg"
        if not path.is_file():
            return None
        svg = read(path)
        # The colour goes on the root element; an icon that already has a fill there gets it replaced (a repeated attribute is an error).
        root_tag = re.match(r"<svg\b[^>]*>", svg).group(0)
        tag = re.sub(r'\sfill="[^"]*"', "", root_tag)
        svg = svg.replace(root_tag, tag.replace("<svg", f'<svg fill="{hexcolor}"', 1), 1)
        dst.mkdir(parents=True, exist_ok=True)
        (dst / f"{stem}.svg").write_text(svg, encoding="utf-8")
        s.used.add(stem)
        return stem

    for n in NAMES:
        if (v := mapping.get(n)) and (i := take(*v)):
            s.names[n] = i
    for e in EXTENSIONS:
        if (v := mapping.get("." + e)) and (i := take(*v)):
            s.exts[e] = i
    s.defaults["file"] = take("default", "white") or take("default", "grey")
    return s


# --- Output ----------------------------------------------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sources", required=True)
    ap.add_argument("--out", default="crates/app")
    args = ap.parse_args()
    out = Path(args.out)
    icons_dir = out / "assets/file-icons"
    if icons_dir.exists():
        shutil.rmtree(icons_dir)
    sets = [
        build_material(args.sources, out),
        build_catppuccin(args.sources, out, "mocha"),
        build_catppuccin(args.sources, out, "latte"),
        build_vscode(args.sources, out),
        build_seti(args.sources, out),
    ]
    rows = []
    for s in sets:
        (icons_dir / f"{s.name}.tsv").write_text(s.tsv(), encoding="utf-8")
        total = sum(p.stat().st_size for p in (icons_dir / s.name).glob("*.svg"))
        print(f"{s.name:18} {len(s.used):4} icons {total // 1024:5} KiB  names {len(s.names)} exts {len(s.exts)} folders {len(s.folders)}", file=sys.stderr)
        for p in sorted((icons_dir / s.name).glob("*.svg")):
            rows.append(f'    ("file-icons/{s.name}/{p.name}", include_bytes!("../assets/file-icons/{s.name}/{p.name}")),')
    # The licence of each project, shipped with the icons (scripts/licenses.sh lists the folder).
    licenses = icons_dir / "LICENSES"
    licenses.mkdir(parents=True, exist_ok=True)
    for name, source in [("material", "material/LICENSE"), ("catppuccin", "catppuccin/LICENSE"), ("vscode-icons", "vscodeicons/LICENSE"),
                         ("seti", "seti/LICENSE.md")]:
        shutil.copyfile(Path(args.sources) / source, licenses / f"{name}-LICENSE")
    code = ["//! Generated by `scripts/icon-sets.py`: the file icons of the icon themes (see docs/file-icons.md). Do not edit.", "",
            "pub(crate) const FILE_ICONS: &[(&str, &[u8])] = &["] + rows + ["];", ""]
    (out / "src/file_icons_assets.rs").write_text("\n".join(code), encoding="utf-8")


if __name__ == "__main__":
    main()
