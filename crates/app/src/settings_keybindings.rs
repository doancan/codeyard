//! `keybindings.jsonc`: read here so `settings.rs` stays within its size budget.

use serde_json::Value;

/// User shortcut: (keys, command id or disable, context).
#[derive(Debug, Clone, PartialEq)]
pub struct UserBinding {
    pub key: String,
    /// `None`: disable this key.
    pub command: Option<String>,
    pub when: Option<String>,
}

pub fn parse_keybindings(text: &str) -> (Vec<UserBinding>, Vec<String>) {
    let mut warnings = Vec::new();
    let value: Value = match serde_json::from_str(&theme::themes::strip_jsonc(text)) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), vec![format!("keybindings.jsonc: {e}")]),
    };
    let Some(items) = value.as_array() else {
        return (Vec::new(), vec![format!("keybindings.jsonc: {}", crate::i18n::t("expected a list [ … ]"))]);
    };
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        // The "cmd+shift+p" / "cmd+k cmd+t" syntax of other editors is accepted too: converted to ours.
        let converted = item.get("key").and_then(Value::as_str).filter(|k| k.contains('+')).and_then(crate::vscode::map_key);
        let key = converted.as_deref().or_else(|| item.get("key").and_then(Value::as_str));
        let command = item.get("command");
        let when = item.get("when").and_then(Value::as_str).map(str::to_string);
        match (key, command) {
            (Some(k), Some(Value::String(c))) => out.push(UserBinding { key: k.into(), command: Some(c.clone()), when }),
            (Some(k), Some(Value::Null)) => out.push(UserBinding { key: k.into(), command: None, when }),
            _ => warnings.push(format!("keybindings.jsonc: {}", crate::i18n::tf("entry {0} needs {1} and {2}", &[&(i + 1), &"\"key\"", &"\"command\""]))),
        }
    }
    (out, warnings)
}
