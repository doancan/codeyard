//! Themes loaded at runtime: embedded themes, user themes
//! (`~/.config/codeyard/themes/*.json`) and imported VS Code color themes.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::{BUILTIN_JSON, DARK, LIGHT, PAIRS, Palette};

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Builtin,
    User(PathBuf),
    /// A VS Code color theme, mapped onto our roles.
    VsCode(PathBuf),
}

/// Why a theme file was rejected. `Display` is English; a UI that localizes matches on the variants.
#[derive(Debug, Clone, PartialEq)]
pub enum ThemeError {
    /// The file could not be read (the OS message).
    Read(String),
    /// Not valid JSON (the parser's message).
    Json(String),
    NotAnObject,
    /// Neither `color` (native) nor `colors`/`tokenColors` (VS Code) is present.
    UnknownFormat,
    ColorNotAnObject,
    ColorNotAString {
        role: String,
    },
    InvalidColor {
        role: String,
        value: String,
    },
    UnknownRole(String),
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeError::Read(e) => f.write_str(e),
            ThemeError::Json(e) => write!(f, "invalid JSON: {e}"),
            ThemeError::NotAnObject => f.write_str("a theme must be a JSON object"),
            ThemeError::UnknownFormat => f.write_str("unrecognized theme format (expected \"color\" or \"colors\")"),
            ThemeError::ColorNotAnObject => f.write_str("\"color\" must be an object"),
            ThemeError::ColorNotAString { role } => write!(f, "{role}: the color must be a string"),
            ThemeError::InvalidColor { role, value } => write!(f, "{role}: invalid color {value}"),
            ThemeError::UnknownRole(role) => write!(f, "unknown role: {role}"),
        }
    }
}

impl std::error::Error for ThemeError {}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Stable id (stored in settings): the file name for embedded themes, "user:<name>" for user themes.
    pub id: String,
    pub name: String,
    pub dark: bool,
    pub palette: Palette,
    pub source: Source,
}

impl Theme {
    /// Pairs still failing AA: (foreground, background, ratio, required).
    pub fn contrast_failures(&self) -> Vec<(&'static str, &'static str, f64, f64)> {
        PAIRS
            .iter()
            .filter_map(|&(fg, bg, min)| {
                let r = contrast(self.palette.get(fg)?, self.palette.get(bg)?);
                (r < min).then_some((fg, bg, r, min))
            })
            .collect()
    }
}

/// Default themes: the two plainest (neutral greys, a single blue accent). Follows the system appearance.
pub const DEFAULT_DARK: &str = "codeyard-dark";
pub const DEFAULT_LIGHT: &str = "codeyard-light";

/// Embedded themes: Midnight/Daylight from the design tokens + themes/*.json.
pub fn builtin() -> Vec<Theme> {
    let mut out = vec![
        Theme { id: "midnight".into(), name: "Midnight".into(), dark: true, palette: guard_contrast(DARK), source: Source::Builtin },
        Theme { id: "daylight".into(), name: "Daylight".into(), dark: false, palette: guard_contrast(LIGHT), source: Source::Builtin },
    ];
    for (id, json) in BUILTIN_JSON {
        match parse(json, Source::Builtin) {
            Ok(mut t) => {
                t.id = (*id).to_string();
                t.palette = guard_contrast(t.palette);
                out.push(t);
            }
            Err(e) => panic!("embedded theme {id}: {e}"),
        }
    }
    // The two defaults come first in the list (the first ones shown in the picker).
    out.sort_by_key(|t| match t.id.as_str() {
        DEFAULT_DARK => 0,
        DEFAULT_LIGHT => 1,
        _ => 2,
    });
    out
}

/// All `.json` themes in a folder. Faulty files are skipped and returned with their errors.
pub fn load_dir(dir: &Path) -> (Vec<Theme>, Vec<(PathBuf, ThemeError)>) {
    let mut themes = Vec::new();
    let mut errors = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return (themes, errors) };
    let mut paths: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    paths.sort();
    for path in paths {
        let result = std::fs::read_to_string(&path).map_err(|e| ThemeError::Read(e.to_string())).and_then(|text| parse(&text, Source::User(path.clone())));
        match result {
            Ok(mut t) => {
                // A theme you wrote yourself (native format) is kept as is; only imported VS Code themes are repaired.
                if matches!(t.source, Source::VsCode(_)) {
                    t.palette = guard_contrast(t.palette);
                }
                t.id = format!("user:{}", path.file_stem().unwrap_or_default().to_string_lossy());
                themes.push(t);
            }
            Err(e) => errors.push((path, e)),
        }
    }
    (themes, errors)
}

/// Our own format (`color` + role names) or a VS Code theme (`colors`, `tokenColors`).
pub fn parse(text: &str, source: Source) -> Result<Theme, ThemeError> {
    let value: Value = serde_json::from_str(&strip_jsonc(text)).map_err(|e| ThemeError::Json(e.to_string()))?;
    let obj = value.as_object().ok_or(ThemeError::NotAnObject)?;
    if obj.contains_key("color") {
        parse_native(obj, source)
    } else if obj.contains_key("colors") || obj.contains_key("tokenColors") {
        let source = match source {
            Source::User(p) => Source::VsCode(p),
            other => other,
        };
        Ok(parse_vscode(obj, source))
    } else {
        Err(ThemeError::UnknownFormat)
    }
}

fn parse_native(obj: &Map<String, Value>, source: Source) -> Result<Theme, ThemeError> {
    let dark = obj.get("appearance").and_then(Value::as_str) != Some("light");
    let mut palette = if dark { DARK } else { LIGHT };
    let colors = obj.get("color").and_then(Value::as_object).ok_or(ThemeError::ColorNotAnObject)?;
    for (role, v) in colors {
        let hex = v.as_str().ok_or_else(|| ThemeError::ColorNotAString { role: role.clone() })?;
        let c = parse_color(hex, palette.bg).ok_or_else(|| ThemeError::InvalidColor { role: role.clone(), value: hex.to_string() })?;
        if !palette.set(role, c) {
            return Err(ThemeError::UnknownRole(role.clone()));
        }
    }
    let name = obj.get("name").and_then(Value::as_str).unwrap_or("Untitled").to_string();
    Ok(Theme { id: String::new(), name, dark, palette, source })
}

/// VS Code key -> our role. The first one found wins.
const VSCODE_MAP: &[(&str, &[&str])] = &[
    ("bg", &["editor.background"]),
    ("surface", &["sideBar.background", "activityBar.background", "panel.background"]),
    ("surface-raised", &["quickInput.background", "editorWidget.background", "menu.background"]),
    ("surface-hover", &["list.hoverBackground", "toolbar.hoverBackground"]),
    ("surface-active", &["list.activeSelectionBackground", "list.inactiveSelectionBackground"]),
    ("current-line", &["editor.lineHighlightBackground"]),
    ("selection", &["editor.selectionBackground"]),
    ("border", &["sideBar.border", "panel.border", "editorGroup.border", "contrastBorder"]),
    ("border-input", &["input.border", "dropdown.border", "contrastBorder"]),
    ("text", &["editor.foreground", "foreground"]),
    ("text-muted", &["descriptionForeground", "tab.inactiveForeground", "sideBarSectionHeader.foreground"]),
    ("line-number", &["editorLineNumber.foreground"]),
    ("line-number-active", &["editorLineNumber.activeForeground", "editor.foreground"]),
    ("accent", &["button.background", "activityBarBadge.background", "focusBorder"]),
    ("on-accent", &["button.foreground", "activityBarBadge.foreground"]),
    ("focus-ring", &["focusBorder", "button.background"]),
    ("danger", &["errorForeground", "editorError.foreground"]),
    ("warning", &["editorWarning.foreground", "list.warningForeground"]),
    ("success", &["gitDecoration.addedResourceForeground", "terminal.ansiGreen"]),
    ("info", &["editorInfo.foreground", "terminal.ansiBlue"]),
    ("diff-added", &["diffEditor.insertedLineBackground", "diffEditor.insertedTextBackground"]),
    ("diff-removed", &["diffEditor.removedLineBackground", "diffEditor.removedTextBackground"]),
];

/// Syntax role -> representative TextMate scope.
const SCOPE_MAP: &[(&str, &str)] = &[
    ("syntax-keyword", "keyword.control"),
    ("syntax-string", "string.quoted.double"),
    ("syntax-number", "constant.numeric"),
    ("syntax-function", "entity.name.function"),
    ("syntax-type", "entity.name.type"),
    ("syntax-comment", "comment.line"),
    ("syntax-tag", "entity.name.tag"),
    ("syntax-punct", "punctuation.separator"),
];

fn parse_vscode(obj: &Map<String, Value>, source: Source) -> Theme {
    let kind = obj.get("type").and_then(Value::as_str).unwrap_or("dark");
    let dark = !kind.contains("light");
    let mut palette = if dark { DARK } else { LIGHT };
    let colors = obj.get("colors").and_then(Value::as_object);
    let lookup = |key: &str| colors.and_then(|c| c.get(key)).and_then(Value::as_str);

    // Background first: colors with alpha are blended over it.
    if let Some(bg) = lookup("editor.background").and_then(|h| parse_color(h, palette.bg)) {
        palette.bg = bg;
    }
    for (role, keys) in VSCODE_MAP {
        if let Some(c) = keys.iter().find_map(|k| lookup(k)).and_then(|h| parse_color(h, palette.bg)) {
            palette.set(role, c);
        }
    }
    if let Some(rules) = obj.get("tokenColors").and_then(Value::as_array) {
        for (role, scope) in SCOPE_MAP {
            if let Some(c) = best_token_color(rules, scope).and_then(|h| parse_color(&h, palette.bg)) {
                palette.set(role, c);
            }
        }
    }
    let name = obj.get("name").and_then(Value::as_str).unwrap_or("VS Code theme").to_string();
    Theme { id: String::new(), name, dark, palette, source }
}

/// TextMate rule: the selector must be a prefix of the scope ("keyword" -> "keyword.control").
/// The selector with the most segments (the most specific) wins; on a tie, the later rule.
fn best_token_color(rules: &[Value], scope: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for rule in rules {
        let Some(fg) = rule.pointer("/settings/foreground").and_then(Value::as_str) else { continue };
        let selectors: Vec<String> = match rule.get("scope") {
            Some(Value::String(s)) => s.split(',').map(|x| x.trim().to_string()).collect(),
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            _ => continue,
        };
        for sel in selectors {
            // For descendant selectors ("source.js keyword"), only the last segment.
            let sel = sel.rsplit(' ').next().unwrap_or(&sel);
            if scope == sel || scope.starts_with(&format!("{sel}.")) {
                let specificity = sel.split('.').count();
                if best.as_ref().is_none_or(|(s, _)| specificity >= *s) {
                    best = Some((specificity, fg.to_string()));
                }
            }
        }
    }
    best.map(|(_, c)| c)
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa` (alpha is blended over `under`).
pub fn parse_color(hex: &str, under: u32) -> Option<u32> {
    let h = hex.trim().strip_prefix('#')?;
    // Slicing is by byte length, so non-ASCII input (emoji, €) would be cut mid-character and panic.
    if !h.is_ascii() {
        return None;
    }
    let expand = |s: &str| -> Option<u32> { u32::from_str_radix(s, 16).ok() };
    let (rgb, alpha) = match h.len() {
        3 | 4 => {
            let full: String = h.chars().flat_map(|c| [c, c]).collect();
            (expand(&full[..6])?, if h.len() == 4 { expand(&full[6..8])? } else { 255 })
        }
        6 => (expand(h)?, 255),
        8 => (expand(&h[..6])?, expand(&h[6..])?),
        _ => return None,
    };
    if alpha == 255 {
        return Some(rgb);
    }
    let a = alpha as f64 / 255.0;
    let mix = |shift: u32| {
        let fg = ((rgb >> shift) & 0xff) as f64;
        let bg = ((under >> shift) & 0xff) as f64;
        ((fg * a + bg * (1.0 - a)).round() as u32) << shift
    };
    Some(mix(16) | mix(8) | mix(0))
}

/// Shift the color toward `toward` in 5% steps until all (background, minimum ratio) constraints are met;
/// if they already are, it is returned unchanged. In the worst case the result is `toward`.
fn nudge(color: u32, toward: u32, constraints: &[(u32, f64)]) -> u32 {
    let ok = |c: u32| constraints.iter().all(|&(bg, min)| contrast(c, bg) >= min);
    if ok(color) {
        return color;
    }
    let mix = |t: f64| {
        let ch = |s: u32| {
            let (a, b) = (((color >> s) & 0xff) as f64, ((toward >> s) & 0xff) as f64);
            ((a + (b - a) * t).round() as u32).min(255) << s
        };
        ch(16) | ch(8) | ch(0)
    };
    (1..=20).map(|i| mix(i as f64 * 0.05)).find(|&c| ok(c)).unwrap_or(toward)
}

fn luminance(c: u32) -> f64 {
    let ch = |s: u32| {
        let v = ((c >> s) & 0xff) as f64 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
}

/// Contrast guard: for each (foreground, background) pair in `PAIRS`, shifts the foreground color by the
/// minimum amount needed toward black (light background) or white (dark background). Backgrounds never
/// change; if the theme author's colors are already sufficient, nothing is touched. This keeps text on
/// hover, selected and accent backgrounds readable in every theme (including third-party and imported ones).
/// `on-accent` is chosen last, based on the corrected accent.
pub fn guard_contrast(mut p: Palette) -> Palette {
    let mut roles: Vec<&'static str> = Vec::new();
    for (fg, _, _) in PAIRS {
        if *fg != "on-accent" && !roles.contains(fg) {
            roles.push(fg);
        }
    }
    // The accent comes after the foreground roles are fixed (it is also used as a fill; `on-accent` is chosen against it).
    roles.sort_by_key(|r| *r == "accent");
    for role in roles {
        let constraints: Vec<(u32, f64)> = PAIRS.iter().filter(|(f, _, _)| *f == role).filter_map(|(_, b, m)| p.get(b).map(|bg| (bg, *m))).collect();
        let Some(current) = p.get(role) else { continue };
        let avg = constraints.iter().map(|(bg, _)| luminance(*bg)).sum::<f64>() / constraints.len().max(1) as f64;
        let toward = if avg > 0.4 { 0x000000 } else { 0xffffff };
        p.set(role, nudge(current, toward, &constraints));
    }
    if contrast(p.on_accent, p.accent) < 4.5 {
        let best = [0xffffff_u32, 0x000000].into_iter().max_by(|a, b| contrast(*a, p.accent).total_cmp(&contrast(*b, p.accent)));
        p.on_accent = best.unwrap_or(p.on_accent);
    }
    p
}

/// WCAG 2 contrast ratio.
pub fn contrast(a: u32, b: u32) -> f64 {
    fn lum(c: u32) -> f64 {
        let ch = |s: u32| {
            let v = ((c >> s) & 0xff) as f64 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
    }
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// JSONC -> JSON: strips `//` and `/* */` comments and trailing commas. Leaves string contents alone.
pub fn strip_jsonc(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut in_str = false;
    // Position of the last comma written: removed if } or ] follows with only whitespace/comments in between.
    let mut comma: Option<usize> = None;
    while i < b.len() {
        let c = b[i];
        if in_str {
            // Copy as a character so multi-byte UTF-8 is not corrupted.
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            if ch == '\\' && i < b.len() {
                let esc = src[i..].chars().next().unwrap();
                out.push(esc);
                i += esc.len_utf8();
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match (c, b.get(i + 1)) {
            (b'"', _) => {
                comma = None;
                in_str = true;
                out.push('"');
                i += 1;
            }
            (b'/', Some(b'/')) => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            (b'/', Some(b'*')) => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            (b',', _) => {
                comma = Some(out.len());
                out.push(',');
                i += 1;
            }
            _ => {
                // Copy as a character so multi-byte UTF-8 is not corrupted.
                let ch = src[i..].chars().next().unwrap();
                if !ch.is_whitespace() {
                    if let (Some(at), '}' | ']') = (comma, ch) {
                        out.replace_range(at..at + 1, " "); // trailing comma
                    }
                    comma = None;
                }
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_with_non_ascii_text_are_rejected_not_panicked_on() {
        for bad in ["#😀", "#aaaaa€", "#€€€", "#ééé", "#aé", "#12345é", "#😀😀😀😀"] {
            assert_eq!(parse_color(bad, 0), None, "{bad}");
        }
        assert_eq!(parse_color("#fff", 0), Some(0xffffff));
        assert_eq!(parse_color("#102030", 0), Some(0x102030));
        assert!(parse_color("#10203080", 0x000000).is_some());
        assert_eq!(parse_color("fff", 0), None);
    }

    #[test]
    fn every_builtin_theme_passes_wcag_aa() {
        let themes = builtin();
        assert!(themes.len() >= 8, "{} tema", themes.len());
        let mut report = String::new();
        for t in &themes {
            for (fg, bg, r, min) in t.contrast_failures() {
                report.push_str(&format!("{}: {fg} on {bg} = {r:.2} (needs {min})\n", t.name));
            }
        }
        assert!(report.is_empty(), "pairs still failing AA:\n{report}");
    }

    #[test]
    fn builtin_ids_are_unique() {
        let themes = builtin();
        let mut ids: Vec<_> = themes.iter().map(|t| t.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), themes.len());
    }

    /// `contrast_failures` skips a pair whose role it cannot read; build.rs already rejects unknown roles, this keeps the skip from ever being silent.
    #[test]
    fn every_contrast_pair_names_real_roles() {
        assert!(PAIRS.len() > 100, "{} pairs", PAIRS.len());
        for (fg, bg, min) in PAIRS {
            assert!(DARK.get(fg).is_some() && DARK.get(bg).is_some(), "pair {fg} / {bg} names an unknown role");
            assert!(*min >= 3.0, "pair {fg} / {bg}: minimum {min} is below WCAG AA for UI");
        }
    }

    /// build.rs embeds whatever `*.json` it finds and embeds nothing if the folder is unreadable; the contrast tests would then check fewer themes
    /// (or none) and still pass. So compare the embedded list with the folder, and refuse files that would be skipped (`.jsonc`, `.JSON`).
    #[test]
    fn every_theme_file_in_the_folder_is_embedded_and_checked() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("themes");
        let mut on_disk = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("themes folder") {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name == "README.md" {
                continue;
            }
            assert!(path.extension().is_some_and(|x| x == "json"), "themes/{name}: only lowercase `.json` files are embedded; this one would be skipped");
            on_disk.push(path.file_stem().unwrap().to_string_lossy().into_owned());
        }
        on_disk.sort();
        let mut embedded: Vec<String> = BUILTIN_JSON.iter().map(|(id, _)| id.to_string()).collect();
        embedded.sort();
        assert!(on_disk.len() >= 16, "only {} theme files found", on_disk.len());
        assert_eq!(embedded, on_disk, "themes/*.json and the embedded list differ (stale build? build.rs failed to read the folder?)");
        assert_eq!(builtin().len(), on_disk.len() + 2, "every file plus Midnight and Daylight");
    }

    /// A native theme file that leaves a role out silently inherits the default theme's colour for it, which no contrast test would notice.
    #[test]
    fn every_theme_file_defines_every_role() {
        for (id, json) in BUILTIN_JSON {
            let value: Value = serde_json::from_str(json).unwrap();
            let colors = value.get("color").and_then(Value::as_object).unwrap_or_else(|| panic!("{id}: built-in themes use the native format (\"color\")"));
            let missing: Vec<&&str> = crate::ROLES.iter().filter(|r| !colors.contains_key(**r)).collect();
            assert!(missing.is_empty(), "{id}: roles missing from the file (they would silently take the default theme's colour): {missing:?}");
        }
    }

    /// `builtin()` runs every theme through `guard_contrast`, so `every_builtin_theme_passes_wcag_aa` cannot tell a broken colour in a file from a
    /// repaired one. These themes are authored to pass as written: breaking one of their colours fails here. The others in `REPAIRED_AT_LOAD` were
    /// adapted from upstream palettes and rely on the guard. A new theme file must be listed in one of the two, so the choice is made on purpose.
    #[test]
    fn authored_themes_pass_wcag_aa_before_the_guard_touches_them() {
        const PASS_AS_WRITTEN: &[&str] = &["codeyard-dark", "codeyard-light", "codeyard-one", "high-contrast-dark", "high-contrast-light", "quarry"];
        const REPAIRED_AT_LOAD: &[&str] =
            &["dracula", "ember", "fjord", "github-light", "graphite", "nord", "one-dark", "paper", "solarized-dark", "solarized-light", "studio-dark"];
        for (id, _) in BUILTIN_JSON {
            assert!(PASS_AS_WRITTEN.contains(id) || REPAIRED_AT_LOAD.contains(id), "{id}: add it to PASS_AS_WRITTEN (preferred) or REPAIRED_AT_LOAD");
        }
        for id in PASS_AS_WRITTEN {
            assert!(BUILTIN_JSON.iter().any(|(i, _)| i == id), "{id} is listed but no such theme file exists");
            let t = Theme { id: id.to_string(), name: id.to_string(), dark: true, palette: raw(id), source: Source::Builtin };
            let failing: Vec<String> = t.contrast_failures().iter().map(|(fg, bg, r, min)| format!("{fg} on {bg} = {r:.2} (needs {min})")).collect();
            assert!(failing.is_empty(), "{id}.json fails WCAG AA as written (the load-time guard would hide this):\n{}", failing.join("\n"));
        }
    }

    #[test]
    fn imports_vscode_theme_with_jsonc_alpha_and_scopes() {
        let src = r##"{
          // VS Code themes are JSONC
          "name": "Örnek Tema",
          "type": "dark",
          "colors": {
            "editor.background": "#101010",
            "editor.foreground": "#eeeeee",
            "editor.selectionBackground": "#ffffff40", /* alpha */
            "button.background": "#3366ff",
          },
          "tokenColors": [
            { "scope": "keyword", "settings": { "foreground": "#ff0000" } },
            { "scope": ["keyword.control", "storage"], "settings": { "foreground": "#00ff00" } },
            { "scope": "comment", "settings": { "foreground": "#777777", "fontStyle": "italic" } },
          ]
        }"##;
        let t = parse(src, Source::User("x.json".into())).unwrap();
        assert!(matches!(t.source, Source::VsCode(_)));
        assert_eq!(t.name, "Örnek Tema");
        assert_eq!(t.palette.bg, 0x101010);
        assert_eq!(t.palette.text, 0xeeeeee);
        assert_eq!(t.palette.selection, 0x4c4c4c, "#ffffff %25 + #101010");
        assert_eq!(t.palette.accent, 0x3366ff);
        assert_eq!(t.palette.syntax_keyword, 0x00ff00, "keyword.control is more specific than keyword");
        assert_eq!(t.palette.syntax_comment, 0x777777);
        assert_eq!(t.palette.syntax_string, DARK.syntax_string, "a missing role falls back to the default");
    }

    #[test]
    fn rejects_unknown_role_in_native_theme() {
        let err = parse(r##"{"name":"x","color":{"bgg":"#000"}}"##, Source::Builtin).unwrap_err();
        assert_eq!(err, ThemeError::UnknownRole("bgg".into()));
    }

    #[test]
    fn jsonc_keeps_strings_intact() {
        let s = strip_jsonc(r#"{"a": "http://x//y", /* c */ "b": "ç,]", }"#);
        assert_eq!(serde_json::from_str::<Value>(&s).unwrap()["a"], "http://x//y");
        assert_eq!(serde_json::from_str::<Value>(&s).unwrap()["b"], "ç,]");
        // A comment after a trailing comma: the comma is still dropped.
        let s = strip_jsonc("{\n  \"a\": [1, 2, // son\n  ],\n  // \"b\": 3,\n}\n");
        assert_eq!(serde_json::from_str::<Value>(&s).unwrap()["a"], serde_json::json!([1, 2]));
    }

    fn raw(id: &str) -> Palette {
        let (_, json) = BUILTIN_JSON.iter().find(|(i, _)| *i == id).unwrap();
        parse(json, Source::Builtin).unwrap().palette
    }

    #[test]
    fn the_guard_leaves_the_plain_default_themes_alone() {
        for id in [DEFAULT_DARK, DEFAULT_LIGHT] {
            let (before, after) = (raw(id), guard_contrast(raw(id)));
            let changed: Vec<String> = crate::ROLES
                .iter()
                .filter(|r| before.get(r) != after.get(r))
                .map(|r| format!("{r}: #{:06x} -> #{:06x}", before.get(r).unwrap(), after.get(r).unwrap()))
                .collect();
            assert!(changed.is_empty(), "{id}: the default themes are authored to pass, so the guard must not change them:\n{}", changed.join("\n"));
        }
    }

    #[test]
    fn the_guard_never_touches_backgrounds_and_is_idempotent() {
        let backgrounds = ["bg", "surface", "surface-raised", "surface-hover", "surface-active", "current-line", "selection", "diff-added", "diff-removed"];
        for (id, json) in BUILTIN_JSON {
            let before = parse(json, Source::Builtin).unwrap().palette;
            let once = guard_contrast(before);
            for bg in backgrounds {
                assert_eq!(once.get(bg), before.get(bg), "{id}: {bg} must not move");
            }
            assert_eq!(guard_contrast(once), once, "{id}: guarding twice changes nothing more");
            let t = Theme { id: id.to_string(), name: id.to_string(), dark: false, palette: once, source: Source::Builtin };
            assert!(t.contrast_failures().is_empty(), "{id}: {:?}", t.contrast_failures());
        }
    }

    #[test]
    fn the_guard_repairs_a_deliberately_unreadable_palette() {
        let mut bad = DARK;
        bad.text_muted = 0x303030;
        bad.syntax_comment = 0x1c1c1e;
        bad.danger = 0x202020;
        bad.on_accent = bad.accent;
        let fixed = guard_contrast(bad);
        let t = Theme { id: "bad".into(), name: "bad".into(), dark: true, palette: fixed, source: Source::Builtin };
        assert!(t.contrast_failures().is_empty(), "{:?}", t.contrast_failures());
        assert_eq!(fixed.bg, DARK.bg, "the background is the author's choice");
        assert_ne!(fixed.text_muted, bad.text_muted);
    }

    #[test]
    fn imported_vscode_themes_are_repaired_but_your_own_theme_files_are_not() {
        let dir = std::env::temp_dir().join(format!("codeyard-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Native format: kept as you wrote it (Save Current Colors, a hand-written theme).
        std::fs::write(dir.join("mine.json"), r##"{ "name": "Mine", "appearance": "dark", "color": { "bg": "#101010", "text-muted": "#202020" } }"##).unwrap();
        // VS Code format: unreadable colors are repaired on load.
        std::fs::write(
            dir.join("vsc.json"),
            r##"{ "name": "Murky", "type": "dark", "colors": { "editor.background": "#101010", "editor.foreground": "#202020" }, "tokenColors": [] }"##,
        )
        .unwrap();
        let (themes, errors) = load_dir(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(errors.is_empty(), "{errors:?}");
        let mine = themes.iter().find(|t| t.name == "Mine").expect("native theme");
        let vsc = themes.iter().find(|t| t.name == "Murky").expect("vscode theme");
        assert_eq!(mine.palette.text_muted, 0x202020, "a native theme file is kept exactly as written");
        assert!(vsc.contrast_failures().is_empty(), "an imported VS Code theme is repaired on load: {:?}", vsc.contrast_failures());
        assert_eq!(vsc.palette.bg, 0x101010, "its background is untouched");
        assert_ne!(vsc.palette.text, 0x202020);
    }
}
