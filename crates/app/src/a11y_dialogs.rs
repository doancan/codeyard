//! In-window dialogs for assistive technology: the extension consent sheet, a modal `AlertDialog` with what the
//! package is, where it comes from, what it may do and contributes, and its Install and Cancel buttons. Its buttons act
//! through the sheet's own path (`confirm_consent`, `cancel_consent`), and only on the package the tree showed. The
//! confirmations asked with `Window::prompt` (discard changes, uninstall, remove a host…) are native dialogs on macOS
//! and Windows, which the system exposes itself.

use super::*;
use crate::workbench::ext_glue::{contribution_lines, high_risk_line, permission_lines, source_line};

const CONSENT: NodeId = NodeId(98);
const CONSENT_INSTALL: NodeId = NodeId(99);
const CONSENT_CANCEL: NodeId = NodeId(100);
/// This module's fixed nodes, for the id layout check.
pub(in crate::workbench) const FIXED: [NodeId; 3] = [CONSENT, CONSENT_INSTALL, CONSENT_CANCEL];
/// The sheet's lines, one label each.
pub(in crate::workbench) const DIALOG_LINE: u64 = 0x1d0_0000;
const MAX_LINES: usize = 1_000;

/// The consent sheet while a package waits for an answer.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ConsentSnap {
    /// Which package: its id, version and source, so a button of an older sheet answers nothing.
    key: String,
    pub(crate) title: String,
    /// What a screen reader says with the title: where it comes from, the High-risk line and the warnings.
    pub(crate) summary: String,
    pub(crate) lines: Vec<String>,
    /// 0 Install, 1 Cancel.
    pub(crate) focused: Option<usize>,
}

impl ConsentSnap {
    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>, kids: &mut Vec<NodeId>) {
        let mut dialog = Node::new(Role::AlertDialog);
        dialog.set_label(self.title.clone());
        dialog.set_description(self.summary.clone());
        dialog.set_modal();
        let mut children = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            let id = NodeId(DIALOG_LINE + i as u64);
            let mut n = Node::new(Role::Label);
            n.set_label(line.clone());
            nodes.push((id, n));
            children.push(id);
        }
        for (id, label) in [(CONSENT_INSTALL, t("Install")), (CONSENT_CANCEL, t("Cancel"))] {
            let mut b = Node::new(Role::Button);
            b.set_label(label);
            b.add_action(Action::Click);
            b.add_action(Action::Focus);
            nodes.push((id, b));
            children.push(id);
        }
        dialog.set_children(children);
        nodes.push((CONSENT, dialog));
        kids.push(CONSENT);
    }

    /// The focused button, else the dialog: while the sheet is open it is where the keyboard is.
    pub(super) fn focus_node(&self) -> Option<NodeId> {
        self.focused.map(|i| if i == 0 { CONSENT_INSTALL } else { CONSENT_CANCEL })
    }
}

impl Workbench {
    /// The sheet while it is in front (another tab in front hides it, and it stays pending).
    pub(super) fn a11y_consent(&self, window: &Window) -> Option<ConsentSnap> {
        if !matches!(self.tabs.page, Some(Page::Extension)) || !self.tabs.page_active {
            return None;
        }
        let s = &self.cfg.exts;
        let (p, (install, cancel)) = (s.pending.as_ref()?, s.consent_focus.as_ref()?);
        let m = &p.manifest;
        let source = source_line(&p.source);
        let risk = high_risk_line(m);
        let mut lines = vec![tf("{0} · version {1} · {2} license", &[&m.id, &m.version, &m.license]), source.clone()];
        lines.extend(m.description.clone());
        lines.extend(risk.clone());
        let perms = permission_lines(&m.permissions);
        lines.push(if perms.is_empty() { t("Permissions: none.").to_string() } else { tf("Permissions: {}", &[&perms.join("; ")]) });
        lines.extend(m.permissions.why.as_ref().map(|why| tf("{0} says why: “{1}”", &[&m.id.publisher(), why])));
        let contributes = contribution_lines(m);
        lines.push(t("Contributes").to_string());
        if contributes.is_empty() {
            lines.push(t("Nothing").to_string());
        }
        lines.extend(contributes);
        lines.extend(p.warnings.iter().cloned());
        lines.truncate(MAX_LINES);
        let summary = std::iter::once(source.clone()).chain(risk).chain(p.warnings.iter().cloned()).collect::<Vec<_>>().join(" ");
        let focused = if install.is_focused(window) {
            Some(0)
        } else if cancel.is_focused(window) {
            Some(1)
        } else {
            None
        };
        Some(ConsentSnap { key: format!("{}@{}\t{source}", m.id, m.version), title: tf("Install {}?", &[&m.name]), summary, lines, focused })
    }

    /// A button of the sheet: it answers only the sheet the tree showed (a later sheet for another package is left
    /// alone), exactly as a click on it does.
    pub(super) fn a11y_consent_action(&mut self, req: &ActionRequest, last: Option<&ConsentSnap>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        if !FIXED.contains(&id) && !(DIALOG_LINE..DIALOG_LINE + BLOCK).contains(&id.0) {
            return false;
        }
        let (Some(old), Some(now)) = (last, self.a11y_consent(window)) else { return true };
        if old.key != now.key {
            return true;
        }
        let Some((install, cancel)) = self.cfg.exts.consent_focus.clone() else { return true };
        match (req.action, id) {
            (Action::Click, CONSENT_INSTALL) => self.confirm_consent(window, cx),
            (Action::Click, CONSENT_CANCEL) => self.cancel_consent(window, cx),
            (Action::Focus, CONSENT_INSTALL) => window.focus(&install),
            (Action::Focus, CONSENT_CANCEL) => window.focus(&cancel),
            _ => {}
        }
        true
    }
}

#[cfg(test)]
#[path = "a11y_dialogs_tests.rs"]
mod tests;
