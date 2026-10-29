//! Own settings of the workspace's extra folders (`.vscode/settings.json`,
//! `.codeyard/settings.jsonc`): VS Code's "resource-scoped" settings, i.e. the ones that
//! vary by the file's folder. The first folder's settings are already layered over the global ones;
//! extra folders affect only their own files.
//!
//! Scope: indentation (`editor.tabSize`, `editor.insertSpaces`), saving
//! (`editor.formatOnSave`, `editor.codeActionsOnSave`, `files.trimTrailingWhitespace`,
//! `files.insertFinalNewline`), paste / type (`editor.formatOnPaste`, `editor.formatOnType`)
//! and rulers (`editor.rulers`). `None`: the folder doesn't set it, the global setting applies.

use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FolderSettings {
    pub tab_size: Option<usize>,
    pub insert_spaces: Option<bool>,
    pub format_on_save: Option<bool>,
    pub code_actions_on_save: Option<Vec<String>>,
    pub format_on_paste: Option<bool>,
    pub format_on_type: Option<bool>,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
    pub rulers: Option<Vec<usize>>,
    /// Resolved from the user file and the workspace file as well (`resolve`): nothing falls back to the global
    /// settings, which carry the first folder's own values.
    pub isolated: bool,
}

impl FolderSettings {
    /// The settings of a folder that is not the first one: user < workspace file < this folder's own files. Every value is
    /// filled in, so the first folder's settings never leak into this folder's files.
    pub fn resolve(user: &str, workspace: Option<&str>, folder: &[&str]) -> Self {
        let mut layers = vec![user];
        layers.extend(workspace);
        layers.extend_from_slice(folder);
        let mut s = FolderSettings::parse(&layers);
        let d = crate::settings::Settings::default();
        s.format_on_save.get_or_insert(d.format_on_save);
        s.format_on_paste.get_or_insert(d.format_on_paste);
        s.format_on_type.get_or_insert(d.format_on_type);
        s.trim_trailing_whitespace.get_or_insert(d.trim_trailing_whitespace);
        s.insert_final_newline.get_or_insert(d.insert_final_newline);
        s.code_actions_on_save.get_or_insert(d.code_actions_on_save);
        s.rulers.get_or_insert(d.rulers);
        // `tab_size` / `insert_spaces` stay `None` when nobody sets them: the file's own indentation is guessed.
        s.tab_size = s.tab_size.or(d.tab_size);
        s.insert_spaces = s.insert_spaces.or(d.insert_spaces);
        s.isolated = true;
        s
    }

    /// Layers in order (later overrides earlier): `.vscode/settings.json`, then `.codeyard/settings.jsonc`.
    /// A broken layer is skipped; values of the wrong type are ignored (the global setting stays).
    pub fn parse(layers: &[&str]) -> Self {
        let mut s = FolderSettings::default();
        for text in layers {
            let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&theme::themes::strip_jsonc(text)) else { continue };
            let boolean = |k: &str| o.get(k).and_then(Value::as_bool);
            let number = |k: &str, min: u64, max: u64| o.get(k).and_then(Value::as_u64).filter(|n| (min..=max).contains(n)).map(|n| n as usize);
            s.tab_size = number("editor.tabSize", 1, 16).or(s.tab_size);
            s.insert_spaces = boolean("editor.insertSpaces").or(s.insert_spaces);
            s.format_on_save = boolean("editor.formatOnSave").or(s.format_on_save);
            s.format_on_paste = boolean("editor.formatOnPaste").or(s.format_on_paste);
            s.format_on_type = boolean("editor.formatOnType").or(s.format_on_type);
            s.trim_trailing_whitespace = boolean("files.trimTrailingWhitespace").or(s.trim_trailing_whitespace);
            s.insert_final_newline = boolean("files.insertFinalNewline").or(s.insert_final_newline);
            if let Some(Value::Array(a)) = o.get("editor.rulers")
                && a.iter().all(|v| v.as_u64().is_some_and(|n| (1..=500).contains(&n)))
            {
                let mut v: Vec<usize> = a.iter().filter_map(Value::as_u64).map(|n| n as usize).collect();
                v.sort_unstable();
                v.dedup();
                s.rulers = Some(v);
            }
            match o.get("editor.codeActionsOnSave") {
                Some(Value::Object(m)) => {
                    s.code_actions_on_save = Some(
                        m.iter()
                            .filter(|(k, v)| k.starts_with("source") && !matches!(v, Value::Bool(false)) && v.as_str() != Some("never"))
                            .map(|(k, _)| k.clone())
                            .collect(),
                    )
                }
                Some(Value::Array(a)) if a.iter().all(Value::is_string) => {
                    s.code_actions_on_save = Some(a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                }
                _ => {}
            }
        }
        s
    }

    pub fn is_empty(&self) -> bool {
        *self == FolderSettings::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_layers_override_and_wrong_types_are_ignored() {
        let vscode = r#"{ "editor.tabSize": 2, "editor.insertSpaces": true, "editor.formatOnSave": true, "files.trimTrailingWhitespace": true, "editor.rulers": [100, 80] }"#;
        let ours = r#"{ // yorum
            "editor.tabSize": 8, "editor.formatOnSave": "yes", "editor.codeActionsOnSave": { "source.organizeImports": "explicit", "source.fixAll": false } }"#;
        let s = FolderSettings::parse(&[vscode, ours]);
        assert_eq!(s.tab_size, Some(8), "later layer overrides");
        assert_eq!(s.insert_spaces, Some(true));
        assert_eq!(s.format_on_save, Some(true), "a wrongly typed value is ignored, the earlier one stays");
        assert_eq!(s.trim_trailing_whitespace, Some(true));
        assert_eq!(s.rulers, Some(vec![80, 100]));
        assert_eq!(s.code_actions_on_save, Some(vec!["source.organizeImports".to_string()]));
        assert_eq!(s.insert_final_newline, None);
    }

    #[test]
    fn broken_or_empty_input_gives_no_overrides() {
        assert!(FolderSettings::parse(&[]).is_empty());
        assert!(FolderSettings::parse(&["{ not json", "[1]", "{}"]).is_empty());
        let s = FolderSettings::parse(&[r#"{ "editor.tabSize": 99, "editor.rulers": [0, 5] }"#]);
        assert_eq!((s.tab_size, s.rulers), (None, None), "out-of-range values are dropped");
    }
}
