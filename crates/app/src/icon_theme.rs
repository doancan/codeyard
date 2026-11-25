//! File and folder icon themes. A theme is a set of real icon files from an open-source icon project (Material Icon Theme,
//! Catppuccin Icons, vscode-icons, Seti UI), embedded in the app: `scripts/icon-sets.py` picks the icons the common file names,
//! extensions and folder names need and writes `assets/file-icons/<set>/` with the set's mapping `<set>.tsv`
//! (docs/file-icons.md). This module looks a name up in the mapping and draws the icon.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{AnyElement, IntoElement as _, Styled as _, div, img, px};

use crate::ui::{self, Palette};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconTheme {
    /// Material Icon Theme: colored file icons by language and tool, folders by purpose.
    Material,
    /// Catppuccin Icons: soft pastel icons; the Mocha palette on dark themes and Latte on light ones.
    Catppuccin,
    /// vscode-icons: the long-standing set with the widest coverage of file names and extensions.
    VsCode,
    /// Seti UI: small single-color file icons, no folder icons.
    Seti,
    /// No icons, name only.
    None,
}

/// The order is the picker's order; the first is the default.
pub const ALL: &[IconTheme] = &[IconTheme::Material, IconTheme::Catppuccin, IconTheme::VsCode, IconTheme::Seti, IconTheme::None];

/// The values the setting accepts: the themes, and the values of the icon sets that were replaced, which read as the default theme.
pub const ACCEPTED: &[&str] = &["material", "catppuccin", "vscode", "seti", "none", "codeyard", "glyphs", "badges", "colorful", "minimal", "apple"];

impl IconTheme {
    pub fn id(self) -> &'static str {
        match self {
            IconTheme::Material => "material",
            IconTheme::Catppuccin => "catppuccin",
            IconTheme::VsCode => "vscode",
            IconTheme::Seti => "seti",
            IconTheme::None => "none",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            IconTheme::Material => "Material Icons",
            IconTheme::Catppuccin => "Catppuccin Icons",
            IconTheme::VsCode => "VS Code Icons",
            IconTheme::Seti => "Seti",
            IconTheme::None => crate::i18n::t("None"),
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            IconTheme::Material => crate::i18n::t("Colorful icons by language and tool, folders marked by purpose (the default)"),
            IconTheme::Catppuccin => crate::i18n::t("Soft pastel icons that follow the light or dark theme"),
            IconTheme::VsCode => crate::i18n::t("The classic set with the widest coverage of file names"),
            IconTheme::Seti => crate::i18n::t("Small single-color file icons; folders show only their arrow"),
            IconTheme::None => crate::i18n::t("Names only"),
        }
    }

    pub fn from_id(id: &str) -> Option<IconTheme> {
        ALL.iter().copied().find(|t| t.id() == id)
    }

    /// The theme a setting value names; an old value (a set that was replaced) or an unknown one is the default.
    pub fn from_setting(id: &str) -> IconTheme {
        Self::from_id(id).unwrap_or(ALL[0])
    }

    /// Whether folders get an icon of their own. Without one the tree puts a file's icon in the arrow's column.
    pub fn folder_icons(self) -> bool {
        !matches!(self, IconTheme::Seti)
    }

    fn code(self) -> u8 {
        ALL.iter().position(|t| *t == self).unwrap_or(0) as u8
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(theme: IconTheme) {
    CURRENT.store(theme.code(), Ordering::Relaxed);
}

pub fn current() -> IconTheme {
    ALL.get(CURRENT.load(Ordering::Relaxed) as usize).copied().unwrap_or(ALL[0])
}

/// What one set maps names to. Icon values are embedded asset paths (`file-icons/<set>/<icon>.svg`).
#[derive(Default)]
pub(crate) struct SetMap {
    names: HashMap<&'static str, &'static str>,
    exts: HashMap<&'static str, &'static str>,
    folders: HashMap<&'static str, (&'static str, &'static str)>,
    file: &'static str,
    folder: Option<(&'static str, &'static str)>,
}

/// The sets of the icon files, by directory under `assets/file-icons/`.
pub(crate) const SETS: [&str; 5] = ["material", "catppuccin-mocha", "catppuccin-latte", "vscode", "seti"];

const TABLES: [&str; 5] = [
    include_str!("../assets/file-icons/material.tsv"),
    include_str!("../assets/file-icons/catppuccin-mocha.tsv"),
    include_str!("../assets/file-icons/catppuccin-latte.tsv"),
    include_str!("../assets/file-icons/vscode.tsv"),
    include_str!("../assets/file-icons/seti.tsv"),
];

/// The asset path of an icon, made once: a few hundred small strings that live as long as the app.
fn path(set: &str, icon: &str) -> &'static str {
    Box::leak(format!("file-icons/{set}/{icon}.svg").into_boxed_str())
}

fn parse(set: &'static str, table: &'static str) -> SetMap {
    let mut map = SetMap::default();
    for line in table.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&'static str> = line.split('\t').collect();
        match f.as_slice() {
            ["n", key, icon] => {
                map.names.insert(key, path(set, icon));
            }
            ["e", key, icon] => {
                map.exts.insert(key, path(set, icon));
            }
            ["f", key, closed, open] => {
                map.folders.insert(key, (path(set, closed), path(set, open)));
            }
            ["d", "file", icon] => map.file = path(set, icon),
            ["d", "folder", closed, open] => map.folder = Some((path(set, closed), path(set, open))),
            _ => {}
        }
    }
    map
}

pub(crate) fn maps() -> &'static [SetMap; 5] {
    static MAPS: OnceLock<[SetMap; 5]> = OnceLock::new();
    MAPS.get_or_init(|| std::array::from_fn(|i| parse(SETS[i], TABLES[i])))
}

/// The set of the active theme: Catppuccin follows the light or dark appearance of the color theme.
fn active_map(theme: IconTheme, p: &Palette) -> Option<&'static SetMap> {
    let at = match theme {
        IconTheme::Material => 0,
        IconTheme::Catppuccin => 1 + usize::from(ui::luminance_is_light(p.bg)),
        IconTheme::VsCode => 3,
        IconTheme::Seti => 4,
        IconTheme::None => return None,
    };
    Some(&maps()[at])
}

impl SetMap {
    /// The icon of a file: its whole name first (`Cargo.toml`, `.gitignore`), then the longest extension that is mapped
    /// (`a.test.ts` tries `test.ts`, then `ts`), then the set's plain file icon.
    pub(crate) fn file_asset(&self, name: &str) -> &'static str {
        let lower = name.to_lowercase();
        if let Some(icon) = self.names.get(lower.as_str()) {
            return icon;
        }
        // Every dot but a leading one starts a candidate: the longest comes first.
        let dots = lower.char_indices().filter(|(i, c)| *c == '.' && *i > 0).map(|(i, _)| i);
        dots.filter_map(|i| self.exts.get(&lower[i + 1..])).next().copied().unwrap_or(self.file)
    }

    /// The icon of a folder, closed or open; the set's plain folder when the name is not a known one.
    pub(crate) fn folder_asset(&self, name: &str, open: bool) -> Option<&'static str> {
        let pair = self.folders.get(name.to_lowercase().as_str()).copied().or(self.folder)?;
        Some(if open { pair.1 } else { pair.0 })
    }
}

fn picture(asset: &'static str, size: f32) -> AnyElement {
    img(asset).flex_none().size(px(size)).into_any_element()
}

/// Whitespace of the same width (for the no-icon theme: a small inner gap so names stay aligned).
fn spacer(size: f32) -> AnyElement {
    div().flex_none().w(px((size * 0.25).round())).into_any_element()
}

/// File icon, according to the active icon theme.
pub fn file_icon_el(name: &str, size: f32, p: &Palette) -> AnyElement {
    match active_map(current(), p) {
        Some(map) => picture(map.file_asset(name), size),
        None => spacer(size),
    }
}

/// Folder icon (in the tree); `open`: open folder.
pub fn folder_icon_el(name: &str, open: bool, size: f32, p: &Palette) -> AnyElement {
    match active_map(current(), p).and_then(|map| map.folder_asset(name, open)) {
        Some(asset) => picture(asset, size),
        None => spacer(size),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_are_unique() {
        for t in ALL {
            assert_eq!(IconTheme::from_id(t.id()), Some(*t));
        }
        let mut ids: Vec<_> = ALL.iter().map(|t| t.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len());
        assert_eq!(IconTheme::from_id("nope"), None);
    }

    #[test]
    fn an_old_or_unknown_value_is_the_default_theme() {
        let legacy: Vec<&&str> = ACCEPTED.iter().filter(|v| IconTheme::from_id(v).is_none()).collect();
        assert_eq!(legacy.len(), ACCEPTED.len() - ALL.len(), "every theme is accepted, and the rest are the replaced ones");
        assert!(ALL.iter().all(|t| ACCEPTED.contains(&t.id())));
        for old in legacy.into_iter().chain([&"", &"nope"]) {
            assert_eq!(IconTheme::from_setting(old), ALL[0], "{old}");
        }
        assert_eq!(IconTheme::from_id(crate::settings::Settings::default().icon_theme.as_str()), Some(ALL[0]));
    }

    #[test]
    fn every_theme_has_a_label_and_description() {
        for t in ALL {
            assert!(!t.label().is_empty() && !t.about().is_empty(), "{t:?}");
        }
    }

    /// Every icon a mapping names is embedded, and every embedded icon is named: the generator and the app agree.
    #[test]
    fn the_mappings_and_the_embedded_icons_agree() {
        let embedded: std::collections::HashSet<&str> = crate::file_icons_assets::FILE_ICONS.iter().map(|(p, _)| *p).collect();
        let mut named = std::collections::HashSet::new();
        for map in maps() {
            let pairs = map.folders.values().flat_map(|(a, b)| [*a, *b]).chain(map.folder.iter().flat_map(|(a, b)| [*a, *b]));
            for icon in map.names.values().copied().chain(map.exts.values().copied()).chain(pairs).chain([map.file]) {
                assert!(embedded.contains(icon), "{icon} is mapped but not embedded");
                named.insert(icon);
            }
        }
        for icon in &embedded {
            assert!(named.contains(icon), "{icon} is embedded but nothing maps to it");
        }
        for (path, bytes) in crate::file_icons_assets::FILE_ICONS {
            assert!(bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml"), "{path} is not an SVG");
        }
    }

    /// The image loader drops an icon it cannot parse without a word: every embedded icon must render, and paint something.
    #[gpui::test]
    fn every_embedded_icon_renders_to_visible_pixels(cx: &mut gpui::TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        let mut failures = Vec::new();
        for (path, bytes) in crate::file_icons_assets::FILE_ICONS {
            match renderer.render_pixmap(bytes, gpui::SvgSize::ScaleFactor(1.)) {
                Ok(pixmap) if pixmap.data().chunks(4).any(|px| px[3] > 0) => {}
                Ok(_) => failures.push(format!("{path}: blank")),
                Err(e) => failures.push(format!("{path}: {e}")),
            }
        }
        assert!(failures.is_empty(), "{} icons do not render: {failures:?}", failures.len());
    }

    #[test]
    fn names_resolve_by_whole_name_then_the_longest_extension_then_the_plain_icon() {
        let m = &maps()[0];
        assert_eq!(m.file_asset("main.rs"), "file-icons/material/rust.svg");
        assert_eq!(m.file_asset("MAIN.RS"), m.file_asset("main.rs"), "case does not matter");
        assert_eq!(m.file_asset("Dockerfile"), "file-icons/material/docker.svg", "a whole name");
        assert_eq!(m.file_asset("package.json"), "file-icons/material/nodejs.svg", "a whole name beats the extension");
        assert_ne!(m.file_asset("data.json"), m.file_asset("package.json"));
        assert_eq!(m.file_asset("a.weird"), m.file, "unknown extension: the plain file icon");
        assert_eq!(m.file_asset(".hidden-and-unknown"), m.file, "a leading dot starts no extension");
        assert_eq!(m.file_asset(""), m.file);
        // `a.test.ts` is tried as `test.ts` first, and falls back to `ts`.
        assert_eq!(m.file_asset("a.nonsense.ts"), m.file_asset("a.ts"));
        for map in maps() {
            for name in ["ğüş.TXT", "x.tar.gz", "a..b", "."] {
                let _ = map.file_asset(name);
            }
        }
    }

    #[test]
    fn folders_have_open_and_closed_icons_by_purpose() {
        let m = &maps()[0];
        let (closed, open) = (m.folder_asset("src", false).unwrap(), m.folder_asset("src", true).unwrap());
        assert_ne!(closed, open);
        assert_eq!(m.folder_asset("SRC", false), Some(closed), "case does not matter");
        assert_ne!(m.folder_asset("src", false), m.folder_asset("whatever", false), "a known purpose has its own icon");
        assert_eq!(m.folder_asset("whatever", false), m.folder.map(|f| f.0));
        assert!(maps()[4].folder_asset("src", false).is_none(), "Seti has no folder icons");
    }

    #[test]
    fn catppuccin_follows_the_light_or_dark_appearance() {
        let (dark, light) = (theme::DARK, theme::LIGHT);
        let d = active_map(IconTheme::Catppuccin, &dark).unwrap().file_asset("a.rs");
        let l = active_map(IconTheme::Catppuccin, &light).unwrap().file_asset("a.rs");
        assert!(d.contains("catppuccin-mocha") && l.contains("catppuccin-latte"), "{d} {l}");
        assert!(active_map(IconTheme::None, &dark).is_none());
    }

    #[test]
    fn every_theme_draws_every_kind_of_name_without_panicking() {
        let p = theme::DARK;
        let names = ["main.rs", "Cargo.lock", "Dockerfile", ".gitignore", "README", "a.weird", "ğüş.TXT", "", "x.tar.gz", "image.PNG"];
        let dirs = ["src", "tests", "node_modules", ".github", "whatever", "", "Ünicode"];
        for t in ALL {
            set(*t);
            for n in names {
                let _ = file_icon_el(n, 16., &p);
            }
            for d in dirs {
                let _ = folder_icon_el(d, false, 16., &p);
                let _ = folder_icon_el(d, true, 16., &p);
            }
        }
        set(ALL[0]);
    }

    #[test]
    fn only_seti_leaves_folders_bare() {
        let bare: Vec<_> = ALL.iter().filter(|t| !t.folder_icons()).collect();
        assert_eq!(bare, [&IconTheme::Seti]);
    }

    #[test]
    fn current_theme_is_switchable() {
        set(IconTheme::VsCode);
        assert_eq!(current(), IconTheme::VsCode);
        set(ALL[0]);
    }
}
