//! The text behind "Help: Copy Diagnostics": what a maintainer needs to understand a report, with everything personal removed.
//! The user copies it and pastes it where they choose; nothing is sent. The report holds:
//!
//! * the version, the build's commit when it is known, the operating system and CPU, the interface language and system locale;
//! * the settings that differ from the defaults, limited to switches, choices and numbers (no text and no paths);
//! * the last notifications (severity, source, message) and the last lines of the log.
//!
//! The home folder, credentials and the user part of `user@host` are removed from all of it (see `diagnostics_redact.rs`);
//! host names stay because the problem is often about a host.

use super::redact::redact;

/// Notifications in the report, newest first.
pub const NOTIFICATIONS: usize = 20;
/// Log lines in the report.
pub const LOG_LINES: usize = 100;
/// Settings listed; a profile that changes everything still gives a report of bounded size.
const MAX_SETTINGS: usize = 80;

/// One notification as the report shows it.
pub struct NoticeLine {
    pub severity: &'static str,
    pub source: String,
    pub message: String,
}

/// Everything the report is made of; the caller reads it from the running app.
pub struct Inputs {
    pub version: &'static str,
    /// The build's commit id, when the build recorded one.
    pub commit: Option<&'static str>,
    pub os: &'static str,
    pub arch: &'static str,
    pub language: String,
    pub locales: Vec<String>,
    /// `key = value` for each setting that is not at its default.
    pub settings: Vec<(String, String)>,
    /// Newest first.
    pub notifications: Vec<NoticeLine>,
    /// Oldest first.
    pub log: Vec<String>,
    pub home: Option<String>,
}

pub fn build(inputs: &Inputs) -> String {
    let mut out = String::new();
    out.push_str("# Codeyard diagnostics\n\n");
    out.push_str(&format!("Version: {}\n", inputs.version));
    out.push_str(&format!("Commit: {}\n", inputs.commit.map_or("not recorded".to_string(), |c| c.chars().take(12).collect())));
    out.push_str(&format!("System: {} {}\n", inputs.os, inputs.arch));
    out.push_str(&format!("Language: {}\n", inputs.language));
    out.push_str(&format!("System locale: {}\n\n", if inputs.locales.is_empty() { "unknown".to_string() } else { inputs.locales.join(", ") }));
    out.push_str("## Settings that are not at their default\n\n");
    if inputs.settings.is_empty() {
        out.push_str("(none)\n");
    }
    for (key, value) in inputs.settings.iter().take(MAX_SETTINGS) {
        out.push_str(&format!("{key} = {value}\n"));
    }
    out.push_str(&format!("\n## Last notifications (newest first, up to {NOTIFICATIONS})\n\n"));
    if inputs.notifications.is_empty() {
        out.push_str("(none)\n");
    }
    for n in inputs.notifications.iter().take(NOTIFICATIONS) {
        out.push_str(&format!("[{}] {}: {}\n", n.severity, n.source, one_line(&n.message)));
    }
    out.push_str(&format!("\n## Log (last {LOG_LINES} lines)\n\n"));
    if inputs.log.is_empty() {
        out.push_str("(empty)\n");
    }
    let skip = inputs.log.len().saturating_sub(LOG_LINES);
    for line in inputs.log.iter().skip(skip) {
        out.push_str(line);
        out.push('\n');
    }
    redact(&out, inputs.home.as_deref())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The settings of `catalog` whose value differs from the default's, as `key = value`. Only switches, choices and numbers are
/// listed: free text can hold a path or a name.
pub fn changed_settings(current: &crate::settings::Settings) -> Vec<(String, String)> {
    use crate::workbench::settings_ui::{CATALOG, Kind, current as value_of};
    let defaults = crate::settings::Settings::default();
    CATALOG
        .iter()
        .filter(|e| !matches!(e.kind, Kind::Text))
        .filter_map(|e| {
            let now = value_of(current, e.key);
            (now != value_of(&defaults, e.key)).then(|| (e.key.to_string(), now.to_string()))
        })
        .collect()
}

#[cfg(test)]
#[path = "diagnostics_report_tests.rs"]
mod tests;
