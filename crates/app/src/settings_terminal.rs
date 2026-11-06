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
    if let Some(v) = r.integer("terminal.persistentSessionTimeout", 0, crate::terminal_session::MAX_SESSION_MINUTES) {
        s.terminal_persistent_session_timeout = v;
    }
    if let Some(v) = r.number("terminal.scrollback", 0., crate::terminal::SCROLLBACK_MAX as f64) {
        s.terminal_scrollback = v as usize;
        if s.terminal_scrollback > crate::terminal::SCROLLBACK_COMFORT {
            r.warn(crate::i18n::tf(
                "terminal.scrollback: {} lines can use hundreds of megabytes per terminal and slow down scrolling and search (up to {} is comfortable); applied as set",
                &[&s.terminal_scrollback, &crate::terminal::SCROLLBACK_COMFORT],
            ));
        }
    }
    if let Some(v) = r.number("terminal.scrollSensitivity", 0.1, 10.) {
        s.terminal_scroll_sensitivity = v as f32;
    }
    if let Some(v) = r.bool("terminal.allowClearHistory") {
        s.terminal_allow_clear_history = v;
    }
    if let Some(v) = r.bool("terminal.allowClipboardWrite") {
        s.terminal_allow_clipboard_write = v;
    }
    if let Some(v) = r.bool("terminal.shellIntegration") {
        s.terminal_shell_integration = v;
    }
    if let Some(v) = r.bool("terminal.notifications") {
        s.terminal_notifications = v;
    }
    if let Some(v) = r.choice("terminal.copyOnSelection", &["agents", "always", "off"]) {
        s.terminal_copy_on_selection = v;
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

#[cfg(test)]
mod confirm_close_default_tests {
    use crate::settings::Settings;

    #[test]
    fn closing_a_terminal_asks_nothing_by_default() {
        assert_eq!(Settings::default().terminal_confirm_close, "never");
    }
}

#[cfg(test)]
mod scrollback_tests {
    use crate::settings::parse;

    #[test]
    fn a_long_history_is_applied_with_a_warning_about_memory() {
        let (s, warnings) = parse(r#"{ "terminal.scrollback": 150000 }"#);
        assert_eq!(s.terminal_scrollback, 150_000);
        assert!(warnings.iter().any(|w| w.contains("terminal.scrollback") && w.contains("150000")), "{warnings:?}");
    }

    #[test]
    fn the_default_and_comfortable_values_do_not_warn() {
        assert_eq!(parse("{}").0.terminal_scrollback, 50_000);
        let (s, warnings) = parse(r#"{ "terminal.scrollback": 50000 }"#);
        assert_eq!(s.terminal_scrollback, 50_000);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn more_than_the_maximum_is_refused() {
        let (s, warnings) = parse(r#"{ "terminal.scrollback": 500000 }"#);
        assert_eq!(s.terminal_scrollback, 50_000, "the default stays");
        assert!(!warnings.is_empty());
    }
}
