//! The editor's hover and signature popups for assistive technology. Both are read-only `Tooltip`s under the editor
//! (polite live regions, so they are spoken when they appear or change), with the painted frame of the popup. The hover
//! holds a line per diagnostic, related location (a `Link` whose `Click` opens it, as a click on it does), "… more"
//! line, the language server's text and each extension's text. `ShowTooltip` on the editor asks for the hover at the
//! caret (as "Editor: Show Hover" does) and `HideTooltip` closes it, so the text is reachable without the pointer.

use super::*;
use crate::editor::PopupsA11y;

const HOVER: NodeId = NodeId(149);
const SIGNATURE: NodeId = NodeId(150);
/// This module's fixed nodes, for the id layout check.
pub(super) const FIXED: [NodeId; 2] = [HOVER, SIGNATURE];
/// The hover's lines (`HOVER_LINE + i`).
pub(super) const HOVER_LINE: u64 = 0x130_0000;
/// Hover lines exposed at most (the popup shows a few dozen).
const MAX_LINES: usize = 200;

/// The editor's popup nodes (children of the editor node).
pub(super) fn append(popups: &PopupsA11y, nodes: &mut Vec<(NodeId, Node)>, children: &mut Vec<NodeId>) {
    if let Some(hover) = &popups.hover {
        let mut node = Node::new(Role::Tooltip);
        node.set_label(t("Hover"));
        node.set_live(Live::Polite);
        if let Some(b) = hover.bounds {
            node.set_bounds(b);
        }
        let mut lines = Vec::new();
        for (i, line) in hover.lines.iter().take(MAX_LINES).enumerate() {
            let id = NodeId(HOVER_LINE + i as u64);
            let mut n = Node::new(if line.opens { Role::Link } else { Role::Label });
            n.set_label(line.text.clone());
            if line.opens {
                n.add_action(Action::Click);
            }
            if let Some(b) = line.bounds {
                n.set_bounds(b);
            }
            nodes.push((id, n));
            lines.push(id);
        }
        node.set_children(lines);
        nodes.push((HOVER, node));
        children.push(HOVER);
    }
    if let Some(sig) = &popups.signature {
        let mut node = Node::new(Role::Tooltip);
        node.set_label(t("Signature help"));
        node.set_value(sig.label.clone());
        if let Some(active) = &sig.active {
            node.set_description(tf("Current parameter: {}", &[active]));
        }
        node.set_live(Live::Polite);
        if let Some(b) = sig.bounds {
            node.set_bounds(b);
        }
        nodes.push((SIGNATURE, node));
        children.push(SIGNATURE);
    }
}

impl Workbench {
    /// `ShowTooltip`/`HideTooltip` on the editor and `Click` on a hover link; `false` if the request is not one.
    pub(super) fn a11y_popup_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let line = (HOVER_LINE..HOVER_LINE + BLOCK).contains(&req.target_node.0);
        let on_editor = req.target_node == EDITOR && matches!(req.action, Action::ShowTooltip | Action::HideTooltip);
        if !line && !on_editor {
            return false;
        }
        let Some(ed) = self.active_editor().cloned() else { return true };
        if last.editor.as_ref().is_none_or(|e| e.name.as_str() != ed.read(cx).name.as_ref()) {
            return true;
        }
        match req.action {
            Action::ShowTooltip => ed.update(cx, |e, cx| e.a11y_show_hover(window, cx)),
            Action::HideTooltip => ed.update(cx, |e, cx| e.a11y_hide_popups(cx)),
            Action::Click if line => {
                let i = (req.target_node.0 - HOVER_LINE) as usize;
                // Only the same line, still a link, at the same place in the same hover.
                let Some(was) = last.popups.as_ref().and_then(|p| p.hover.as_ref()).and_then(|h| h.lines.get(i)) else { return true };
                if was.opens {
                    let text = was.text.clone();
                    ed.update(cx, |e, cx| e.a11y_open_hover_line(i, &text, cx));
                }
            }
            _ => {}
        }
        true
    }
}
