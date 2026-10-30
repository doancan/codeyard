//! The `terminal.*` settings: read here so `settings.rs` stays within its size budget.

use serde_json::{Map, Value};

use super::{Reader, Settings};

pub(super) fn parse(s: &mut Settings, r: &mut Reader<'_>, obj: &Map<String, Value>) {
    s.terminal_shell = r.string("terminal.shell");
    if let Some(v) = r.number("terminal.fontSize", 6., 48.) {
        s.terminal_font_size = v as f32;
    }
    if let Some(v) = r.number("terminal.lineHeight", 1., 2.) {
        s.terminal_line_height = v as f32;
    }
    s.terminal_persistent_scrollback = r.integer("terminal.persistentScrollback", 0, crate::terminal_session::MAX_HISTORY_LINES as u64).map_or(0, |v| v as _);
    if let Some(v) = r.number("terminal.scrollback", 0., crate::terminal::SCROLLBACK_MAX as f64) {
        s.terminal_scrollback = v as usize;
    }
    if let Some(v) = r.number("terminal.scrollSensitivity", 0.1, 10.) {
        s.terminal_scroll_sensitivity = v as f32;
    }
    if let Some(v) = r.bool("terminal.allowClipboardWrite") {
        s.terminal_allow_clipboard_write = v;
    }
    if let Some(v) = r.bool("terminal.shellIntegration") {
        s.terminal_shell_integration = v;
    }
    if let Some(v) = r.choice("terminal.confirmOnClose", &["running", "always", "never"]) {
        s.terminal_confirm_close = v;
    }
    s.terminal_default_profile = r.string("terminal.defaultProfile").filter(|p| !p.trim().is_empty());
    for os in ["macos", "linux", "windows"] {
        let key = format!("terminal.profiles.{os}");
        match obj.get(&key) {
            None | Some(Value::Null) => {}
            Some(Value::Object(m)) => {
                let mut w = Vec::new();
                s.terminal_profiles.insert(os.to_string(), crate::terminal_profiles::parse_profiles(&key, m, &mut w));
                for w in w {
                    r.warn(w);
                }
            }
            Some(_) => r.warn(crate::i18n::tf("{0}: expected an object {1}", &[&key, &"{ \"zsh\": { \"path\": \"/bin/zsh\" } }"])),
        }
    }
    if let Some(v) = r.choice("terminal.sanitizePaste", &["auto", "always", "off"]) {
        s.terminal_sanitize_paste = crate::terminal_paste::Sanitize::parse(&v).unwrap_or_default();
    }
    if let Some(v) = r.choice("terminal.warnMultilinePaste", &["unbracketed", "always", "never"]) {
        s.terminal_warn_multiline = crate::terminal_paste::WarnMultiline::parse(&v).unwrap_or_default();
    }
}
