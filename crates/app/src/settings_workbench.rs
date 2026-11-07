//! Settings for the workbench's trust, panel and save behaviour: read here so `settings.rs` stays within its size budget.

use super::{Reader, Settings};

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
}
