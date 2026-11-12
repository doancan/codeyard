//! Settings for the workbench's trust, panel and save behaviour: read here so `settings.rs` stays within its size budget.

use super::{Reader, Settings};

/// Settings read here that are not in the main list of known keys.
pub(super) const KEYS: &[&str] = &["explorer.fileNesting.enabled", "explorer.fileNesting.patterns", "diagnostics.logLevel"];

pub(super) fn parse(s: &mut Settings, r: &mut Reader<'_>) {
    if let Some(v) = r.bool("files.trimFinalNewlines") {
        s.trim_final_newlines = v;
    }
    if let Some(v) = r.choice("security.workspaceTrust.startupPrompt", &["once", "always", "never"]) {
        s.workspace_trust_prompt = v;
    }
    if let Some(v) = r.bool("workbench.panel.firstTerminalMaximized") {
        s.panel_first_terminal_maximized = v;
    }
    if let Some(v) = r.choice("workbench.panel.openFromMaximized", &["half", "restore", "tab"]) {
        s.panel_open_from_maximized = v;
    }
    if let Some(v) = r.choice("diagnostics.logLevel", &crate::diagnostics::log::Level::NAMES) {
        s.log_level = crate::diagnostics::log::Level::parse(&v).unwrap_or(s.log_level);
    }
    parse_nesting(s, r);
}

/// `explorer.fileNesting.enabled` and `.patterns`: files shown under the file they belong to.
pub(super) fn parse_nesting(s: &mut Settings, r: &mut Reader<'_>) {
    let enabled = r.bool("explorer.fileNesting.enabled");
    let patterns = r.get("explorer.fileNesting.patterns").cloned();
    if enabled.is_none() && patterns.is_none() {
        return;
    }
    let enabled = enabled.unwrap_or(true);
    s.explorer_nesting = match patterns {
        None => crate::tree::nesting::Nesting::default().with_enabled(enabled),
        Some(value) => {
            let (nesting, warnings) = crate::tree::nesting::Nesting::from_setting(enabled, &value);
            for w in warnings {
                r.warn(format!("explorer.fileNesting.patterns: {w}"));
            }
            nesting
        }
    };
}
