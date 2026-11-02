//! The Extension Inspector's AccessKit nodes: a group per page with a heading per extension and section, one list item
//! per line (a call, a denial, a log line, a trap frame) and a **Copy fix** button after each denial that has a fix.
//! Like the Extensions view, an action runs only when the line still says what the screen reader was told.

use super::*;
use crate::workbench::inspector_glue::{InspectorLine, LineKind};

/// The Inspector while it is in front.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct InspectorSnap {
    pub lines: Vec<InspectorLine>,
}

/// Line `i` is `INSPECTOR_LINE + 2 i`; its **Copy fix** button the id after it.
pub(super) fn line_id(i: usize) -> NodeId {
    NodeId(INSPECTOR_LINE + 2 * i as u64)
}

pub(super) fn append_tree(snap: &InspectorSnap, nodes: &mut Vec<(NodeId, Node)>) {
    let mut group = Node::new(Role::Group);
    group.set_label(t("Extension Inspector"));
    let mut children = Vec::new();
    for (i, l) in snap.lines.iter().enumerate() {
        let id = line_id(i);
        let mut n = Node::new(match l.kind {
            LineKind::Extension | LineKind::Section => Role::Heading,
            LineKind::Row => Role::ListItem,
        });
        if l.kind == LineKind::Extension {
            n.set_level(2);
        } else if l.kind == LineKind::Section {
            n.set_level(3);
        }
        n.set_label(l.text.clone());
        n.add_action(Action::Focus);
        nodes.push((id, n));
        children.push(id);
        if let Some(fix) = &l.fix {
            let bid = NodeId(id.0 + 1);
            let mut b = Node::new(Role::Button);
            b.set_label(t("Copy fix"));
            b.set_description(fix.clone());
            b.add_action(Action::Click);
            nodes.push((bid, b));
            children.push(bid);
        }
    }
    group.set_children(children);
    nodes.push((INSPECTOR, group));
}

impl Workbench {
    pub(in crate::workbench) fn a11y_inspector(&self) -> Option<InspectorSnap> {
        if !matches!(self.tabs.page, Some(Page::ExtensionInspector)) || !self.tabs.page_active {
            return None;
        }
        Some(InspectorSnap { lines: self.inspector_lines() })
    }

    pub(super) fn a11y_inspector_action(&mut self, req: &ActionRequest, last: &Snapshot, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        if id != INSPECTOR && !(INSPECTOR_LINE..INSPECTOR_LINE + BLOCK).contains(&id.0) {
            return false;
        }
        let Some(snap) = &last.inspector else { return true };
        let offset = (id.0.saturating_sub(INSPECTOR_LINE)) as usize;
        if req.action == Action::Click
            && offset % 2 == 1
            && let Some(line) = snap.lines.get(offset / 2)
        {
            self.inspector_copy_fix(offset / 2, line, cx);
        }
        true
    }
}

#[cfg(test)]
#[path = "a11y_inspector_tests.rs"]
mod tests;
