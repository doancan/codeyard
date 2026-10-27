//! File and folder icon themes. Colors come from the active color theme's roles
//! (icons adapt when the theme changes); no custom image file is shipped, badges are drawn.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{AnyElement, IntoElement as _, ParentElement as _, Styled as _, div, px};

use crate::ui::{self, Palette, c, icon};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconTheme {
    /// Lucide (ISC) file icons, colored by language; folders in the accent color.
    Default,
    /// Small badge colored by language ("RS", "TS", "PY"…); folders are muted.
    Badges,
    /// Colored folders (src, test, docs…) and colored file icons.
    Colorful,
    /// Single-color (muted) line icons.
    Minimal,
    /// macOS Finder / GitHub: filled blue folders, line file icon, language dot at the bottom right.
    Apple,
    /// Material: filled folders colored by purpose (small icon inside), filled files colored by language.
    Material,
    /// Colored file glyphs, no folder icons: a folder shows only its arrow and a file's glyph takes the arrow's column, so every
    /// name in a level lines up.
    Glyphs,
    /// No icons, name only.
    None,
}

/// The order is the picker's order; the first is the default.
pub const ALL: &[IconTheme] = &[
    IconTheme::Glyphs,
    IconTheme::Minimal,
    IconTheme::Default,
    IconTheme::Badges,
    IconTheme::Colorful,
    IconTheme::Apple,
    IconTheme::Material,
    IconTheme::None,
];

impl IconTheme {
    pub fn id(self) -> &'static str {
        match self {
            IconTheme::Default => "codeyard",
            IconTheme::Badges => "badges",
            IconTheme::Colorful => "colorful",
            IconTheme::Minimal => "minimal",
            IconTheme::Apple => "apple",
            IconTheme::Material => "material",
            IconTheme::Glyphs => "glyphs",
            IconTheme::None => "none",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            IconTheme::Default => "Codeyard",
            IconTheme::Badges => crate::i18n::t("Language Badges"),
            IconTheme::Colorful => crate::i18n::t("Colorful Folders"),
            IconTheme::Minimal => crate::i18n::t("Minimal"),
            IconTheme::Apple => "Apple / GitHub",
            IconTheme::Material => "Material",
            IconTheme::Glyphs => crate::i18n::t("File Glyphs (Default)"),
            IconTheme::None => crate::i18n::t("None"),
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            IconTheme::Default => crate::i18n::t("File glyphs tinted by language, accent folders"),
            IconTheme::Badges => crate::i18n::t("Small colored badges with the language's letters"),
            IconTheme::Colorful => crate::i18n::t("Folders colored by purpose (src, tests, docs…) and colored file glyphs"),
            IconTheme::Minimal => crate::i18n::t("One quiet color, outline glyphs"),
            IconTheme::Apple => crate::i18n::t("Solid blue folders like Finder, outline files with a language dot like GitHub"),
            IconTheme::Material => crate::i18n::t("Solid folders colored and marked by purpose, solid files colored by language"),
            IconTheme::Glyphs => crate::i18n::t("Colored file glyphs, folders with only their arrow; names line up"),
            IconTheme::None => crate::i18n::t("Names only"),
        }
    }

    pub fn from_id(id: &str) -> Option<IconTheme> {
        ALL.iter().copied().find(|t| t.id() == id)
    }

    /// Whether folders get an icon of their own. Without one the tree puts a file's icon in the arrow's column.
    pub fn folder_icons(self) -> bool {
        !matches!(self, IconTheme::Glyphs)
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

fn ext_of(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Badge text: the language's 1-2 letters. Unknown extension: the first two letters of the extension; with no extension "·".
pub fn badge_text(name: &str) -> String {
    let ext = ext_of(name);
    let known = match ext.as_str() {
        "rs" => "RS",
        "ts" | "mts" | "cts" => "TS",
        "tsx" => "TX",
        "js" | "mjs" | "cjs" => "JS",
        "jsx" => "JX",
        "py" | "pyi" => "PY",
        "go" => "GO",
        "c" => "C",
        "h" | "hpp" | "hh" => "H",
        "cpp" | "cc" | "cxx" => "C+",
        "java" => "JV",
        "kt" | "kts" => "KT",
        "swift" => "SW",
        "rb" => "RB",
        "php" => "PH",
        "lua" => "LU",
        "zig" => "ZG",
        "html" | "htm" => "<>",
        "vue" => "VU",
        "svelte" => "SV",
        "css" => "CS",
        "scss" | "sass" => "SC",
        "less" => "LE",
        "json" | "jsonc" => "{}",
        "toml" => "TM",
        "yaml" | "yml" => "YM",
        "md" | "mdx" => "MD",
        "txt" => "TX",
        "sh" | "bash" | "zsh" | "fish" => "$",
        "sql" => "SQ",
        "lock" => "LK",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp" => "IM",
        "svg" => "SV",
        "xml" => "XM",
        "csv" => "CV",
        "ini" | "conf" | "env" => "CF",
        "log" => "LG",
        "wasm" => "WA",
        _ => "",
    };
    if !known.is_empty() {
        return known.into();
    }
    match name {
        "Dockerfile" => "DK".into(),
        "Makefile" | "Justfile" => "MK".into(),
        "LICENSE" | "LICENCE" => "LI".into(),
        _ if ext.is_empty() => "·".into(),
        _ => ext.chars().take(2).collect::<String>().to_uppercase(),
    }
}

/// The file's language color (from the palette role).
pub fn file_tint(name: &str, p: &Palette) -> u32 {
    ui::file_icon(name, p).1
}

/// Folder name's purpose → color (Colorful theme).
pub fn folder_tint(name: &str, p: &Palette) -> u32 {
    match name.to_ascii_lowercase().as_str() {
        "src" | "source" | "sources" | "lib" | "crates" | "app" | "packages" => p.success,
        "test" | "tests" | "spec" | "specs" | "__tests__" | "e2e" => p.warning,
        "doc" | "docs" | "documentation" | "wiki" => p.info,
        "config" | "configs" | ".config" | ".github" | ".vscode" | ".codeyard" | ".idea" | "scripts" | "bin" | ".circleci" => p.syntax_keyword,
        "build" | "dist" | "out" | "target" | "node_modules" | "vendor" | ".git" | "venv" | ".venv" | "__pycache__" => p.text_muted,
        "assets" | "images" | "img" | "public" | "static" | "media" | "icons" | "fonts" => p.syntax_string,
        "components" | "ui" | "views" | "pages" | "widgets" => p.syntax_type,
        _ => p.accent,
    }
}

fn badge(text: String, size: f32, color: u32, p: &Palette) -> AnyElement {
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(3.))
        .bg(ui::ca(color, 0.2))
        .border_1()
        .border_color(ui::ca(color, 0.55))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .text_size(px(size * if text.chars().count() > 1 { 0.5 } else { 0.62 }))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(c(if ui::luminance_is_light(p.bg) { ui::darken(color) } else { color }))
        .child(text)
        .into_any_element()
}

/// Whitespace of the same width (for the no-icon theme: a small inner gap so names stay aligned).
fn spacer(size: f32) -> AnyElement {
    div().flex_none().w(px((size * 0.25).round())).into_any_element()
}

/// File icon, according to the active icon theme.
pub fn file_icon_el(name: &str, size: f32, p: &Palette) -> AnyElement {
    match current() {
        IconTheme::Default | IconTheme::Colorful | IconTheme::Glyphs => {
            let (glyph, tint) = ui::file_icon(name, p);
            icon(glyph, size, tint).into_any_element()
        }
        IconTheme::Badges => badge(badge_text(name), size, file_tint(name, p), p),
        IconTheme::Minimal => {
            let (glyph, _) = ui::file_icon(name, p);
            icon(glyph, size, p.text_muted).into_any_element()
        }
        IconTheme::Apple => {
            let (_, tint) = ui::file_icon(name, p);
            let dot = (tint != p.text_muted).then_some(tint);
            div()
                .relative()
                .flex_none()
                .size(px(size))
                .child(icon("file", size, p.text_muted))
                .children(dot.map(|t| {
                    let d = (size * 0.4).round();
                    div().absolute().right(px(-1.)).bottom(px(-1.)).size(px(d)).rounded_full().bg(c(t)).border_1().border_color(c(p.bg))
                }))
                .into_any_element()
        }
        IconTheme::Material => {
            let (glyph, tint) = ui::file_icon(name, p);
            let mark = (glyph != "file")
                .then(|| div().absolute().inset_0().pt(px(size * 0.18)).flex().items_center().justify_center().child(icon(glyph, size * 0.5, p.bg)));
            div().relative().flex_none().size(px(size)).child(icon("file-solid", size, tint)).children(mark).into_any_element()
        }
        IconTheme::None => spacer(size),
    }
}

/// The small icon inside a Material folder (by purpose).
fn folder_mark(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "src" | "source" | "sources" | "lib" | "crates" | "app" | "packages" => Some("code"),
        "test" | "tests" | "spec" | "specs" | "__tests__" | "e2e" => Some("check"),
        "doc" | "docs" | "documentation" | "wiki" => Some("book-open"),
        "config" | "configs" | ".config" | ".github" | ".vscode" | ".codeyard" | ".idea" | ".circleci" => Some("settings"),
        "scripts" | "bin" => Some("terminal"),
        "assets" | "images" | "img" | "public" | "static" | "media" | "icons" | "fonts" => Some("file-image"),
        "build" | "dist" | "out" | "target" => Some("rocket"),
        ".git" => Some("git-branch"),
        _ => None,
    }
}

/// Folder icon (in the tree); `open`: open folder.
pub fn folder_icon_el(name: &str, open: bool, size: f32, p: &Palette) -> AnyElement {
    let glyph = if open { "folder-open" } else { "folder" };
    match current() {
        IconTheme::Default => icon(glyph, size, p.accent).into_any_element(),
        IconTheme::Colorful => icon(glyph, size, folder_tint(name, p)).into_any_element(),
        IconTheme::Badges | IconTheme::Minimal => icon(glyph, size, p.text_muted).into_any_element(),
        IconTheme::Apple => icon(if open { "folder-open-solid" } else { "folder-solid" }, size, p.info).into_any_element(),
        IconTheme::Material => {
            let base = if open { "folder-open-solid" } else { "folder-solid" };
            let mark = folder_mark(name)
                .filter(|_| !open)
                .map(|g| div().absolute().inset_0().pt(px(size * 0.12)).flex().items_center().justify_center().child(icon(g, size * 0.46, p.bg)));
            div().relative().flex_none().size(px(size)).child(icon(base, size, folder_tint(name, p))).children(mark).into_any_element()
        }
        IconTheme::None | IconTheme::Glyphs => spacer(size),
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
    fn badges_use_the_language_letters() {
        assert_eq!(badge_text("main.rs"), "RS");
        assert_eq!(badge_text("App.TSX"), "TX");
        assert_eq!(badge_text("Cargo.lock"), "LK");
        assert_eq!(badge_text("Dockerfile"), "DK");
        assert_eq!(badge_text("a.weird"), "WE");
        assert_eq!(badge_text("Procfile"), "·");
    }

    #[test]
    fn folders_are_colored_by_purpose() {
        let p = theme::DARK;
        assert_eq!(folder_tint("src", &p), p.success);
        assert_eq!(folder_tint("Tests", &p), p.warning);
        assert_eq!(folder_tint("node_modules", &p), p.text_muted);
        assert_eq!(folder_tint("whatever", &p), p.accent);
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
    fn the_default_icon_theme_marks_files_and_leaves_folders_bare() {
        assert_eq!(ALL[0], IconTheme::Glyphs);
        assert_eq!(IconTheme::from_id(crate::settings::Settings::default().icon_theme.as_str()), Some(IconTheme::Glyphs));
    }

    #[test]
    fn material_folders_have_marks_only_for_known_purposes() {
        assert_eq!(folder_mark("src"), Some("code"));
        assert_eq!(folder_mark("Docs"), Some("book-open"));
        assert_eq!(folder_mark("whatever"), None);
    }

    #[test]
    fn every_theme_has_a_label_and_description() {
        for t in ALL {
            assert!(!t.label().is_empty() && !t.about().is_empty(), "{:?}", t);
        }
    }

    #[test]
    fn only_the_glyphs_theme_leaves_folders_bare() {
        let bare: Vec<_> = ALL.iter().filter(|t| !t.folder_icons()).collect();
        assert_eq!(bare, [&IconTheme::Glyphs]);
    }

    #[test]
    fn current_theme_is_switchable() {
        set(IconTheme::Minimal);
        assert_eq!(current(), IconTheme::Minimal);
        set(ALL[0]);
    }
}
