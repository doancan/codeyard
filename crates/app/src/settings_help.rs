//! The "?" help of a setting. The text is data, not code: `locales/help/<code>.tsv` has one line per setting
//! (`key<TAB>text`, `\n` for a line break), one file per UI language, registered in `i18n::LANGUAGES`. A setting without a line
//! in the current language shows the English one; without any, only the facts the page works out from the setting itself
//! ([`facts`]). Adding or translating help never touches Rust code (docs/settings-help.md).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use crate::workbench::settings_ui::Kind;

/// `key → text` of one help file. `#` starts a comment line.
pub fn parse(table: &str) -> HashMap<String, String> {
    table
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('\t'))
        .map(|(key, text)| (key.trim().to_string(), text.replace("\\n", "\n")))
        .collect()
}

/// The parsed help of every language, in `LANGUAGES` order.
fn tables() -> &'static [HashMap<String, String>] {
    static TABLES: OnceLock<Vec<HashMap<String, String>>> = OnceLock::new();
    TABLES.get_or_init(|| crate::i18n::LANGUAGES.iter().map(|l| parse(l.help)).collect())
}

/// The explanation of `key` in the UI language, else in English.
pub fn help_for(key: &str) -> Option<&'static str> {
    let all = tables();
    all.get(crate::i18n::current()).and_then(|t| t.get(key)).or_else(|| all.first().and_then(|t| t.get(key))).map(String::as_str)
}

/// What the page knows about a setting without anyone writing it down: the key as it is written in settings.jsonc, the default and
/// what values are valid. Lines in the UI language.
pub fn facts(key: &str, kind: Option<Kind>, default: &Value) -> Vec<String> {
    use crate::i18n::{t, tf};
    let mut lines = vec![tf("Key in settings.jsonc: {}", &[&key])];
    match kind {
        Some(Kind::Choice(options)) => lines.push(tf("Values: {}", &[&options.join(", ")])),
        Some(Kind::Number { min, max }) => lines.push(tf("Values: a number from {} to {}", &[&min, &max])),
        Some(Kind::Bool) => lines.push(tf("Values: {}", &[&t("on or off")])),
        _ => {}
    }
    let shown = match default {
        Value::Null => t("none").to_string(),
        Value::String(s) if s.is_empty() => t("empty").to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    lines.push(tf("Default: {}", &[&shown]));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{LANGUAGES, LangGuard};
    use crate::workbench::settings_ui::CATALOG;

    #[test]
    fn a_help_file_is_lines_of_key_tab_text_with_comments_and_line_breaks() {
        let t = parse("# comment\n\nui.language\tOne\\nTwo\nbroken line without a tab\n");
        assert_eq!(t.len(), 1);
        assert_eq!(t["ui.language"], "One\nTwo");
    }

    /// Every help line belongs to a setting the page shows, and no setting is listed twice (a duplicate would silently win).
    #[test]
    fn every_help_line_names_a_setting_of_the_page_once() {
        for l in LANGUAGES {
            let keys: Vec<&str> = l.help.lines().filter(|x| !x.trim().is_empty() && !x.starts_with('#')).filter_map(|x| x.split('\t').next()).collect();
            for k in &keys {
                assert!(CATALOG.iter().any(|e| e.key == *k), "{}: help for “{k}”, which is not a setting of the page", l.code);
            }
            let mut sorted = keys.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), keys.len(), "{}: a setting has two help lines", l.code);
        }
    }

    /// A translation explains settings the English file explains, and keeps the code words (`on`, `loopback`, setting keys) that
    /// appear in backticks, so a translated text cannot name a value the setting does not have.
    #[test]
    fn a_translation_matches_the_english_help() {
        let en = parse(LANGUAGES[0].help);
        for l in LANGUAGES.iter().skip(1) {
            for (k, text) in parse(l.help) {
                let source = en.get(&k).unwrap_or_else(|| panic!("{}: “{k}” has no English help", l.code));
                let code = |s: &str| {
                    let mut v: Vec<String> = s.split('`').skip(1).step_by(2).map(str::to_string).collect();
                    v.sort();
                    v
                };
                assert_eq!(code(&text), code(source), "{}: the `code` words of “{k}” differ from the English", l.code);
            }
        }
    }

    #[test]
    fn the_core_settings_of_languages_are_explained_in_every_language() {
        for k in ["ui.language", "spelling.enabled", "lsp.enabled", "security.workspaceTrust.enabled"] {
            for l in LANGUAGES.iter().filter(|l| l.status != crate::i18n::Status::Source) {
                assert!(parse(l.help).contains_key(k), "{}: no help for {k}", l.code);
            }
        }
    }

    #[test]
    fn help_follows_the_ui_language_and_falls_back_to_english() {
        assert!(help_for("ui.language").unwrap().starts_with("The language of menus"));
        {
            let _g = LangGuard::set("tr");
            assert!(help_for("ui.language").unwrap().starts_with("Menülerin"));
        }
        assert_eq!(help_for("editor.fontSize"), None, "no line, no help text");
    }

    #[test]
    fn the_facts_name_the_key_the_values_and_the_default() {
        let f = facts("editor.wordWrap", Some(Kind::Choice(&["off", "on"])), &Value::String("off".into()));
        assert_eq!(f, ["Key in settings.jsonc: editor.wordWrap", "Values: off, on", "Default: off"]);
        let f = facts("editor.tabSize", Some(Kind::Number { min: 1., max: 16. }), &Value::Null);
        assert_eq!(f[1..], ["Values: a number from 1 to 16", "Default: none"]);
    }
}
