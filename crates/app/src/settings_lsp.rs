//! Settings of the language-server features: the `lsp.servers` table entries and the refactoring preview (read here so
//! `settings.rs` stays within its size budget).

use serde_json::Value;

use super::{Reader, Settings};
use crate::lsp::refactor::plan::ReviewMode;

/// Settings read here that are not in the main list of known keys.
pub(super) const KEYS: [&str; 1] = ["editor.refactorPreview"];

pub(super) fn parse(s: &mut Settings, r: &mut Reader<'_>) {
    if let Some(v) = r.choice("editor.refactorPreview", &ReviewMode::CHOICES) {
        s.refactor_preview = ReviewMode::parse(&v).unwrap_or_default();
    }
}

/// One `lsp.servers` entry: `Some(None)` turns the language off, `None` is malformed. An object needs at least one of
/// `command`, `settings` and `initializationOptions`; a `settings` value that is not an object is warned about and the rest
/// of the entry still applies.
pub(super) fn server_entry(lang: &str, cfg: &Value, warnings: &mut Vec<String>) -> Option<Option<crate::lsp::ServerSpec>> {
    let o = match cfg {
        Value::Null => return Some(None),
        Value::String(cmd) if !cmd.trim().is_empty() => return Some(Some((cmd.clone(), Vec::new()).into())),
        Value::Object(o) => o,
        _ => return None,
    };
    let command = match o.get("command") {
        None => None,
        Some(Value::String(c)) if !c.trim().is_empty() => Some(c.clone()),
        Some(_) => return None,
    };
    if command.is_none() && !o.contains_key("settings") && !o.contains_key("initializationOptions") {
        return None;
    }
    let args = o.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let settings = match o.get("settings") {
        None | Some(Value::Null) => Value::Null,
        Some(v @ Value::Object(_)) => crate::lsp::expand_dotted(v),
        Some(_) => {
            warnings.push(format!("settings.jsonc: {}", crate::i18n::tf("{}: expected an object", &[&format!("lsp.servers.{lang}.settings")])));
            Value::Null
        }
    };
    let init_options = o.get("initializationOptions").filter(|v| !v.is_null()).cloned();
    Some(Some(crate::lsp::ServerSpec { command, args, settings: std::sync::Arc::new(settings), init_options, program: None }))
}
