//! Single source of the settings reference: `workbench_settings::CATALOG` (changeable from the UI) and `FILE_ONLY` (set only
//! in the file). `docs/settings.md` is generated from them; if a setting is added to neither, a test fails.

use crate::settings::Settings;
use crate::workbench::settings_ui::{CATALOG, Kind, current};

/// Settings not shown in the Settings page but valid in `settings.jsonc`: (key, value format, default, description).
pub const FILE_ONLY: &[(&str, &str, &str, &str)] = &[
    ("theme", "theme id or `auto`", "`auto`", "Colour theme; `auto` follows the system light/dark setting. Picked with the theme picker (⌘K ⌘T)."),
    ("editor.rulers", "list of columns", "`[]`", "Draw vertical rulers at these columns, e.g. `[80, 120]`."),
    ("files.exclude", "list of globs", "`[]`", "Hide matching paths from the explorer and from file lists, e.g. `[\"**/node_modules\"]`."),
    ("search.exclude", "list of globs", "`[]`", "Skip matching paths in Find in Files."),
    (
        "compare.folders.ignore",
        "list of globs",
        "`[\".git\", \"node_modules\", \"target\"]`",
        "Names and paths *Compare: Folders…* leaves out on both sides (gitignore syntax).",
    ),
    (
        "compare.folders.maxFiles",
        "number 100–5000000",
        "`50000`",
        "Entries per side *Compare: Folders…* reads before it stops and reports that the result is partial.",
    ),
    ("workbench.startupEditor", "string", "`welcome`", "VS Code compatible alias of `window.startup`."),
    (
        "editor.codeActionsOnSave",
        "object",
        "`{}`",
        "Code actions to run on save before formatting, e.g. `{ \"source.organizeImports\": \"explicit\" }`; each gives up after 2 s.",
    ),
    ("files.associations", "object of glob → language", "`{}`", "Force a language mode for matching files, e.g. `{ \"*.tpl\": \"html\" }`."),
    (
        "editor.formatters",
        "object of language → formatter",
        "`{}`",
        "Who formats a language: `\"lsp\"` or the id of an extension that formats it, e.g. `{ \"toml\": \"you.taplo\" }`. Without an entry the language server formats, or else the only extension that formats the language.",
    ),
    (
        "terminal.profiles.macos",
        "object of name → profile",
        "`{}`",
        "Profiles for terminals on macOS: `{ \"zsh\": { \"path\": \"/bin/zsh\", \"args\": [\"-l\"], \"env\": { … }, \"color\": \"info\" } }`; `args` replace the login flag, `color` is accent, success, warning, danger, info or muted. Needs a trusted folder in a project file.",
    ),
    ("terminal.profiles.linux", "object of name → profile", "`{}`", "Profiles for terminals on Linux (also SSH hosts), like `terminal.profiles.macos`."),
    ("terminal.profiles.windows", "object of name → profile", "`{}`", "Profiles for terminals on Windows, like `terminal.profiles.macos`."),
    (
        "ports.rules",
        "object of port or range → rule",
        "`{}`",
        "Label, protocol (`http`, `https`, `tcp`) and detection behaviour (`notify`, `silent`, `openBrowser`, `ignore`) per forwarded port, e.g. `{ \"3000\": { \"label\": \"web\", \"onForward\": \"openBrowser\" } }`; a single port beats a range.",
    ),
    (
        "json.schemas",
        "list of `{ fileMatch, url }`",
        "`[]`",
        "Associate JSON files with a schema (URL or path relative to the workspace). Ignored in untrusted folders.",
    ),
    ("yaml.schemas", "object of schema → globs", "`{}`", "Associate YAML files with a schema. Ignored in untrusted folders."),
    (
        "lsp.servers",
        "object of language → `{ command, args, settings, initializationOptions }` or `null`",
        "built-in servers",
        "Replace, configure or turn off (`null`) the language server of a language. `settings` is what the server reads as its configuration (dotted keys are nested objects; a change reaches a running server without a restart), `initializationOptions` is sent when it starts; without `command` the default server is used. Keys: `rust`, `c`, `go`, `python`, `typescript`, `java`, `csharp`, `php`, `ruby`, `kotlin`, `swift`, `lua`, `bash`, `html`, `css`; `json`, `yaml` and `toml` have no default and are opt-in. Ignored in untrusted folders.",
    ),
    (
        "extensions.registries",
        "list of `{ name, url, root-keys, threshold }`",
        "`[]`",
        "Extension registries besides the public one, e.g. a company's own `yard`: a short `name`, its `https://` address, the hex public keys that sign its `root.json` and how many of them must (default: all). User settings only.",
    ),
    ("spelling.languages", "list of dictionary ids", "`[\"en\"]`", "Dictionaries used together; install more with *Spelling: Install Dictionary…*."),
    ("rust.features", "list of feature names", "`[]`", "Cargo features Run and Debug build with, besides a target's required features."),
    ("rust.noDefaultFeatures", "boolean", "`false`", "Build Run and Debug targets without the package's default features."),
    ("rust.profile", "string", "Cargo's default (`dev`)", "Cargo profile Run and Debug build with, e.g. `release` or a custom `[profile.*]`."),
    (
        "rust.targetDir",
        "path",
        "Cargo's own",
        "Output folder for Run and Debug builds (`--target-dir`); a relative path is resolved against the workspace folder on the machine that builds, also over SSH.",
    ),
    ("workbench.colorCustomizations", "object", "`{}`", "Override single colour roles; an entry in `[Theme Name]` applies to that theme only."),
];

fn kind_text(k: &Kind) -> String {
    match k {
        Kind::Bool => "`true` / `false`".into(),
        Kind::Choice(o) => o.iter().map(|v| format!("`{v}`")).collect::<Vec<_>>().join(", "),
        Kind::Number { min, max } => format!("number {min}–{max}"),
        Kind::Text => "text".into(),
    }
}

/// Settings whose default depends on the platform: (key, macOS value, Windows and Linux value). The reference lists both,
/// so it reads the same whichever platform generates it.
pub(crate) const PLATFORM_DEFAULTS: &[(&str, f32, f32)] = &[
    ("editor.fontSize", theme::text::CODE_MACOS.0, theme::text::CODE_OTHER.0),
    ("editor.lineHeight", theme::text::CODE_MACOS.1, theme::text::CODE_OTHER.1),
    ("terminal.fontSize", theme::text::CODE_MACOS.0, theme::text::CODE_OTHER.0),
];

fn default_text(key: &str) -> String {
    if let Some((_, mac, other)) = PLATFORM_DEFAULTS.iter().find(|(k, ..)| *k == key) {
        return format!("`{mac}` on macOS, `{other}` on Windows and Linux");
    }
    // Tests run with the trust check and language servers off and the language forced to `en`; the shipped defaults are
    // on and `auto`.
    let s = Settings { workspace_trust: true, lsp_enabled: true, ui_language: "auto".into(), ..Settings::default() };
    match current(&s, key) {
        serde_json::Value::Null => "detected".into(),
        serde_json::Value::String(t) if t.is_empty() => "empty".into(),
        serde_json::Value::String(t) => format!("`{t}`"),
        v => format!("`{v}`"),
    }
}

/// Full text of `docs/settings.md`.
pub fn reference_markdown() -> String {
    let mut out = String::from(
        "# Settings reference\n\n\
         <!-- Generated from crates/app/src/workbench_settings.rs and settings_docs.rs. Do not edit by hand:\n\
         run `UPDATE_DOCS=1 cargo test -p app docs_reference` after changing a setting. -->\n\n\
         Settings live in `~/.config/codeyard/settings.jsonc` (comments and trailing commas are fine); a folder can override them in\n\
         `.codeyard/settings.jsonc` or `.vscode/settings.json`. Every setting below can also be changed in the Settings page (⌘,)\n\
         unless it is listed under *File-only settings*. A wrong value never breaks the app: that key keeps its default and the\n\
         status bar reports it.\n",
    );
    let mut groups: Vec<&str> = Vec::new();
    for e in CATALOG {
        let g = e.key.split('.').next().unwrap_or(e.key);
        if !groups.contains(&g) {
            groups.push(g);
        }
    }
    groups.sort_unstable();
    for g in groups {
        out.push_str(&format!("\n## `{g}`\n\n| Setting | Values | Default | What it does |\n| --- | --- | --- | --- |\n"));
        let mut rows: Vec<_> = CATALOG.iter().filter(|e| e.key.split('.').next() == Some(g)).collect();
        rows.sort_by_key(|e| e.key);
        for e in rows {
            out.push_str(&format!("| `{}` | {} | {} | {} |\n", e.key, kind_text(&e.kind), default_text(e.key), e.about.replace('|', "\\|")));
        }
    }
    out.push_str("\n## File-only settings\n\n| Setting | Values | Default | What it does |\n| --- | --- | --- | --- |\n");
    for (k, v, d, a) in FILE_ONLY {
        out.push_str(&format!("| `{k}` | {v} | {d} | {a} |\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs").join(name)
    }

    /// `docs/settings.md` is generated; the file in the repository must equal the generated text.
    #[test]
    fn docs_reference_settings_is_up_to_date() {
        let want = reference_markdown();
        let path = docs_path("settings.md");
        if std::env::var_os("UPDATE_DOCS").is_some() {
            std::fs::write(&path, &want).unwrap();
            return;
        }
        // A Windows checkout may have turned the file's `\n` into `\r\n`.
        let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        assert!(have == want, "docs/settings.md is out of date: run `UPDATE_DOCS=1 cargo test -p app docs_reference`");
    }

    /// The platform table agrees with the real defaults on this platform, so the reference cannot drift from them.
    #[test]
    fn docs_reference_platform_defaults_match_this_platform() {
        let s = Settings::default();
        for (key, mac, other) in PLATFORM_DEFAULTS {
            let here = if cfg!(target_os = "macos") { mac } else { other };
            assert_eq!(current(&s, key), serde_json::json!(here), "{key}");
        }
    }

    /// Every key the settings parser reads is documented, either in the catalog or in `FILE_ONLY`.
    #[test]
    fn docs_reference_covers_every_parsed_setting() {
        let src = concat!(include_str!("settings.rs"), include_str!("settings_terminal.rs"), include_str!("settings_rust.rs"));
        let mut parsed = std::collections::BTreeSet::new();
        for m in ["bool", "choice", "number", "integer", "numbers", "strings", "string", "code_actions", "scale"] {
            let needle = format!("r.{m}(\"");
            let mut rest = src;
            while let Some(i) = rest.find(&needle) {
                rest = &rest[i + needle.len()..];
                if let Some(end) = rest.find('"') {
                    parsed.insert(rest[..end].to_string());
                }
            }
        }
        // A new reader method (`r.something("key")`) that is not in the list above would hide its keys from this test: fail instead.
        let known = ["bool", "choice", "number", "integer", "numbers", "strings", "string", "code_actions", "scale"];
        let mut unknown_methods = std::collections::BTreeSet::new();
        for (at, _) in src.match_indices("r.") {
            if src[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                continue;
            }
            let rest = &src[at + 2..];
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if rest[name.len()..].starts_with("(\"") && !known.contains(&name.as_str()) {
                unknown_methods.insert(name);
            }
        }
        assert!(
            unknown_methods.is_empty(),
            "settings.rs reads keys through reader methods this test does not scan: {unknown_methods:?} (add them to the list)"
        );
        for k in [
            "files.associations",
            "editor.formatters",
            "workbench.colorCustomizations",
            "lsp.servers",
            "extensions.registries",
            "spelling.languages",
            "json.schemas",
            "yaml.schemas",
        ] {
            parsed.insert(k.to_string());
        }
        assert!(parsed.len() > 80, "the scan found only {} keys; did the parser change shape?", parsed.len());
        let missing: Vec<_> = parsed.iter().filter(|k| !CATALOG.iter().any(|e| e.key == k.as_str()) && !FILE_ONLY.iter().any(|f| f.0 == k.as_str())).collect();
        assert!(missing.is_empty(), "settings parsed but not documented (add to CATALOG or settings_docs::FILE_ONLY): {missing:?}");
        let unknown: Vec<_> = FILE_ONLY.iter().filter(|f| !src.contains(&format!("\"{}\"", f.0))).map(|f| f.0).collect();
        assert!(unknown.is_empty(), "documented as file-only but the parser does not read: {unknown:?}");
    }
}
