//! The Refactor Preview page's AccessKit nodes: the page, a live status, the files, file operations and changes as a tree of
//! check boxes (a bounded window of rows around the keyboard row), and the Apply, Discard and Undo buttons. Actions are checked
//! against the snapshot they were made from: a row is only acted on if it still reads the same.

use super::*;
use crate::lsp::refactor::preview::{AccessView, RefactorPreview};
use crate::replace_plan::Tri;

pub(super) const REFACTOR: NodeId = NodeId(450);
pub(super) const REFACTOR_STATUS: NodeId = NodeId(451);
pub(super) const REFACTOR_LIST: NodeId = NodeId(452);
pub(super) const REFACTOR_APPLY: NodeId = NodeId(453);
pub(super) const REFACTOR_DISCARD: NodeId = NodeId(454);
pub(super) const REFACTOR_UNDO: NodeId = NodeId(455);
pub(super) const REFACTOR_REPORT: NodeId = NodeId(456);
/// Rows of the tree (ids `REFACTOR_ROW + i`, i < `MAX_ROWS_SHOWN`).
pub(super) const REFACTOR_ROW: u64 = 0x210_0000;
/// Rows exposed around the keyboard row; the rest are counted in the set size.
const MAX_ROWS_SHOWN: usize = 200;

/// The Refactor Preview in front.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct RefactorSnap {
    entity: gpui::EntityId,
    pub(crate) view: AccessView,
}

impl RefactorSnap {
    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>) {
        let v = &self.view;
        let b = &v.bounds;
        let place = |node: &mut Node, bounds: Option<accesskit::Rect>| {
            if let Some(r) = bounds {
                node.set_bounds(r);
            }
        };
        let mut group = Node::new(Role::Group);
        group.set_label(v.title.clone());
        group.set_description(v.summary.clone());
        if v.busy {
            group.set_busy();
        }
        place(&mut group, b.page);
        let mut children = Vec::new();

        let mut status = Node::new(if v.problem { Role::Alert } else { Role::Status });
        status.set_label(v.status.clone());
        status.set_live(if v.problem { Live::Assertive } else { Live::Polite });
        status.set_live_atomic();
        place(&mut status, b.status);
        nodes.push((REFACTOR_STATUS, status));
        children.push(REFACTOR_STATUS);

        if !v.report.is_empty() {
            let mut report = Node::new(Role::Status);
            report.set_label(v.report.join(" "));
            report.set_live(Live::Polite);
            report.set_live_atomic();
            nodes.push((REFACTOR_REPORT, report));
            children.push(REFACTOR_REPORT);
        }

        if v.rows_total > 0 {
            let mut list = Node::new(Role::Tree);
            list.set_label(t("Refactor Preview"));
            list.set_size_of_set(v.rows_total);
            list.add_action(Action::Focus);
            place(&mut list, b.list);
            let mut ids = Vec::new();
            for (i, row) in v.rows.iter().enumerate() {
                let id = NodeId(REFACTOR_ROW + i as u64);
                let mut o = Node::new(Role::TreeItem);
                o.set_label(row.label.clone());
                if let Some(d) = row.detail.as_ref().filter(|d| !d.is_empty()) {
                    o.set_description(d.clone());
                }
                o.set_level(row.level);
                o.set_selected(v.selected == Some(v.first_row + i));
                o.set_position_in_set(v.first_row + i + 1);
                o.set_size_of_set(v.rows_total);
                if let Some(expanded) = row.expanded {
                    o.set_expanded(expanded);
                    o.add_action(if expanded { Action::Collapse } else { Action::Expand });
                }
                if let Some(state) = row.toggled {
                    o.set_toggled(match state {
                        Tri::All => Toggled::True,
                        Tri::Some => Toggled::Mixed,
                        Tri::None => Toggled::False,
                    });
                }
                o.add_action(Action::Click);
                o.add_action(Action::Focus);
                place(&mut o, b.rows.get(i).copied().flatten());
                ids.push(id);
                nodes.push((id, o));
            }
            list.set_children(ids);
            nodes.push((REFACTOR_LIST, list));
            children.push(REFACTOR_LIST);
        }

        if let Some((label, enabled)) = &v.apply {
            let mut apply = Node::new(Role::Button);
            apply.set_label(label.clone());
            if *enabled {
                apply.add_action(Action::Click);
            } else {
                apply.set_disabled();
            }
            place(&mut apply, b.apply);
            nodes.push((REFACTOR_APPLY, apply));
            children.push(REFACTOR_APPLY);
        }
        let mut discard = Node::new(Role::Button);
        discard.set_label(v.discard_label.clone());
        discard.add_action(Action::Click);
        place(&mut discard, b.discard);
        nodes.push((REFACTOR_DISCARD, discard));
        children.push(REFACTOR_DISCARD);
        if v.can_undo {
            let mut undo = Node::new(Role::Button);
            undo.set_label(t("Undo"));
            undo.add_action(Action::Click);
            place(&mut undo, b.undo);
            nodes.push((REFACTOR_UNDO, undo));
            children.push(REFACTOR_UNDO);
        }
        group.set_children(children);
        nodes.push((REFACTOR, group));
    }

    /// The node that has focus: the keyboard row while the tree is focused.
    pub(super) fn focus_node(&self) -> Option<NodeId> {
        let v = &self.view;
        if !v.list_focused {
            return None;
        }
        Some(match v.selected.and_then(|s| s.checked_sub(v.first_row)).filter(|i| *i < v.rows.len()) {
            Some(i) => NodeId(REFACTOR_ROW + i as u64),
            None => REFACTOR_LIST,
        })
    }
}

impl Workbench {
    /// The Refactor Preview in front, if any.
    pub(super) fn front_refactor(&self) -> Option<Entity<RefactorPreview>> {
        match &self.tabs.page {
            Some(Page::Refactor(view)) if self.tabs.page_active => Some(view.clone()),
            _ => None,
        }
    }

    pub(super) fn a11y_refactor_snap(&self, window: &Window, cx: &App) -> Option<RefactorSnap> {
        let view = self.front_refactor()?;
        Some(RefactorSnap { entity: view.entity_id(), view: view.read(cx).access_view(MAX_ROWS_SHOWN, window) })
    }

    /// Handle an action aimed at a Refactor Preview node; `false` if it is not one.
    pub(super) fn a11y_refactor_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        let ours = (REFACTOR.0..=REFACTOR_REPORT.0).contains(&id.0) || (REFACTOR_ROW..REFACTOR_ROW + BLOCK).contains(&id.0);
        if !ours {
            return false;
        }
        let (Some(snap), Some(view)) = (&last.refactor, self.front_refactor()) else { return true };
        if snap.entity != view.entity_id() {
            return true;
        }
        let current = view.read(cx).access_view(MAX_ROWS_SHOWN, window);
        match (req.action, id) {
            (Action::Focus, REFACTOR_LIST) => view.update(cx, |v, cx| v.focus_list(window, cx)),
            (Action::Click, REFACTOR_APPLY) => {
                if current.apply == snap.view.apply && current.apply.as_ref().is_some_and(|(_, on)| *on) {
                    view.update(cx, |v, cx| v.request_apply(cx));
                }
            }
            (Action::Click, REFACTOR_DISCARD) => {
                if current.discard_label == snap.view.discard_label {
                    view.update(cx, |v, cx| v.request_discard(cx));
                }
            }
            (Action::Click, REFACTOR_UNDO) if snap.view.can_undo && current.can_undo => view.update(cx, |v, cx| v.request_undo(cx)),
            (Action::Focus | Action::Click | Action::Expand | Action::Collapse, id) if (REFACTOR_ROW..REFACTOR_ROW + BLOCK).contains(&id.0) => {
                let i = (id.0 - REFACTOR_ROW) as usize;
                let Some(expected) = snap.view.rows.get(i) else { return true };
                let ix = snap.view.first_row + i;
                if view.read(cx).row_words(ix) != *expected {
                    return true;
                }
                view.update(cx, |v, cx| {
                    v.focus_list(window, cx);
                    match req.action {
                        Action::Click if expected.toggled.is_some() => v.toggle_row(ix, cx),
                        Action::Click => v.activate_row(ix, cx),
                        Action::Expand => v.fold_row(ix, false, cx),
                        Action::Collapse => v.fold_row(ix, true, cx),
                        _ => v.select_row(ix, cx),
                    }
                });
            }
            _ => {}
        }
        cx.notify();
        true
    }
}

#[cfg(test)]
#[path = "a11y_refactor_tests.rs"]
mod tests;
