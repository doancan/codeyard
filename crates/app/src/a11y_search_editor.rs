//! The Search Editor's AccessKit nodes: its tab, the query, a live status, the results (or the replacement preview) as a
//! list box with a bounded window of rows around the keyboard row, and the Apply / Undo buttons. Actions are checked
//! against the snapshot they were made from: a row is only activated if it still has the same label.

use super::*;
use crate::search_editor::{AccessView, SearchEditor};

pub(super) const SEARCH_ED: NodeId = NodeId(32);
pub(super) const SEARCH_ED_QUERY: NodeId = NodeId(33);
pub(super) const SEARCH_ED_STATUS: NodeId = NodeId(34);
pub(super) const SEARCH_ED_LIST: NodeId = NodeId(35);
pub(super) const SEARCH_ED_APPLY: NodeId = NodeId(36);
pub(super) const SEARCH_ED_UNDO: NodeId = NodeId(37);
/// Rows of the list (ids `SEARCH_ED_ROW + i`, i < `MAX_ROWS_SHOWN`) and the Search Editor tabs (`SEARCH_ED_TAB + i`).
pub(super) const SEARCH_ED_ROW: u64 = 0xb0_0000;
pub(super) const SEARCH_ED_TAB: u64 = 0xc0_0000;
/// Rows exposed around the keyboard row; the rest are counted in the set size.
pub(super) const MAX_ROWS_SHOWN: usize = 200;

/// The Search Editor in front.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct SearchEdSnap {
    pub(super) entity: gpui::EntityId,
    pub(crate) view: AccessView,
}

/// A Search Editor tab of the focused group.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct SearchTabSnap {
    pub(super) entity: gpui::EntityId,
    pub(crate) title: String,
    pub(crate) active: bool,
}

impl SearchEdSnap {
    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>) {
        let v = &self.view;
        let mut group = Node::new(Role::Group);
        group.set_label(v.title.clone());
        let mut children = Vec::new();

        let mut query = Node::new(Role::SearchInput);
        query.set_label(t("Search"));
        query.set_value(v.query.clone());
        query.add_action(Action::Focus);
        nodes.push((SEARCH_ED_QUERY, query));
        children.push(SEARCH_ED_QUERY);

        let mut status = Node::new(Role::Status);
        status.set_label(v.status.clone());
        status.set_live(Live::Polite);
        status.set_live_atomic();
        nodes.push((SEARCH_ED_STATUS, status));
        children.push(SEARCH_ED_STATUS);

        let mut list = Node::new(Role::ListBox);
        list.set_label(if v.apply.is_some() { t("Replacement preview") } else { t("Search results") });
        list.set_size_of_set(v.rows_total);
        list.add_action(Action::Focus);
        let mut ids = Vec::new();
        for (i, (label, detail)) in v.rows.iter().enumerate() {
            let id = NodeId(SEARCH_ED_ROW + i as u64);
            let mut o = Node::new(Role::ListBoxOption);
            o.set_label(label.clone());
            if let Some(d) = detail.as_ref().filter(|d| !d.is_empty()) {
                o.set_description(d.clone());
            }
            o.set_selected(v.selected == Some(v.first_row + i));
            o.set_position_in_set(v.first_row + i + 1);
            o.set_size_of_set(v.rows_total);
            o.add_action(Action::Click);
            o.add_action(Action::Focus);
            ids.push(id);
            nodes.push((id, o));
        }
        list.set_children(ids);
        nodes.push((SEARCH_ED_LIST, list));
        children.push(SEARCH_ED_LIST);

        if let Some((label, enabled)) = &v.apply {
            let mut apply = Node::new(Role::Button);
            apply.set_label(label.clone());
            if *enabled {
                apply.add_action(Action::Click);
            } else {
                apply.set_disabled();
            }
            nodes.push((SEARCH_ED_APPLY, apply));
            children.push(SEARCH_ED_APPLY);
        }
        if v.can_undo {
            let mut undo = Node::new(Role::Button);
            undo.set_label(t("Undo Replace"));
            undo.add_action(Action::Click);
            nodes.push((SEARCH_ED_UNDO, undo));
            children.push(SEARCH_ED_UNDO);
        }
        group.set_children(children);
        nodes.push((SEARCH_ED, group));
    }

    /// The node that has focus: the selected row while the list is focused, the query while it is.
    pub(super) fn focus_node(&self) -> Option<NodeId> {
        let v = &self.view;
        if v.query_focused {
            return Some(SEARCH_ED_QUERY);
        }
        if !v.list_focused {
            return None;
        }
        Some(match v.selected.and_then(|s| s.checked_sub(v.first_row)).filter(|i| *i < v.rows.len()) {
            Some(i) => NodeId(SEARCH_ED_ROW + i as u64),
            None => SEARCH_ED_LIST,
        })
    }

    pub(super) fn id() -> NodeId {
        SEARCH_ED
    }
}

impl Workbench {
    /// The Search Editor in front of the focused group, if any.
    pub(super) fn front_search_editor(&self) -> Option<Entity<SearchEditor>> {
        match &self.tabs.page {
            Some(Page::Search(ed)) if self.tabs.page_active => Some(ed.clone()),
            _ => None,
        }
    }

    pub(super) fn a11y_search_snap(&self, window: &Window, cx: &App) -> (Option<SearchEdSnap>, Vec<SearchTabSnap>) {
        let front = self.front_search_editor();
        let snap = front.as_ref().map(|ed| SearchEdSnap { entity: ed.entity_id(), view: ed.read(cx).access_view(MAX_ROWS_SHOWN, window, cx) });
        let tabs = self
            .tabs
            .searches
            .iter()
            .map(|ed| SearchTabSnap { entity: ed.entity_id(), title: ed.read(cx).title(), active: front.as_ref() == Some(ed) })
            .collect();
        (snap, tabs)
    }

    /// Handle an action aimed at a Search Editor node; `false` if it is not one.
    pub(super) fn a11y_search_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        if (SEARCH_ED_TAB..SEARCH_ED_TAB + BLOCK).contains(&id.0) {
            let i = (id.0 - SEARCH_ED_TAB) as usize;
            let Some(tab) = last.search_tabs.get(i) else { return true };
            if let Some(ed) = self.tabs.searches.iter().find(|e| e.entity_id() == tab.entity).cloned() {
                self.show_search_editor(ed, window, cx);
            }
            return true;
        }
        let ours = (SEARCH_ED.0..=SEARCH_ED_UNDO.0).contains(&id.0) || (SEARCH_ED_ROW..SEARCH_ED_ROW + BLOCK).contains(&id.0);
        if !ours {
            return false;
        }
        let (Some(snap), Some(ed)) = (&last.search_editor, self.front_search_editor()) else { return true };
        if snap.entity != ed.entity_id() {
            return true;
        }
        match (req.action, id) {
            (Action::Focus, SEARCH_ED_QUERY) => ed.read(cx).focus_query(window, cx),
            (Action::Focus, SEARCH_ED_LIST) => ed.update(cx, |e, cx| e.focus_list(window, cx)),
            (Action::Click, SEARCH_ED_APPLY) => {
                let current = ed.read(cx).access_view(MAX_ROWS_SHOWN, window, cx);
                if current.apply == snap.view.apply && current.apply.as_ref().is_some_and(|(_, on)| *on) {
                    self.apply_replace(ed, cx);
                }
            }
            (Action::Click, SEARCH_ED_UNDO) if snap.view.can_undo => self.undo_replace(cx),
            (Action::Focus | Action::Click, id) if (SEARCH_ED_ROW..SEARCH_ED_ROW + BLOCK).contains(&id.0) => {
                let i = (id.0 - SEARCH_ED_ROW) as usize;
                let Some(expected) = snap.view.rows.get(i) else { return true };
                let ix = snap.view.first_row + i;
                if ed.read(cx).row_label(ix) != *expected {
                    return true;
                }
                ed.update(cx, |e, cx| {
                    e.focus_list(window, cx);
                    if req.action == Action::Click {
                        e.activate_row(ix, cx);
                    } else {
                        e.select_row(ix, cx);
                    }
                });
            }
            _ => {}
        }
        cx.notify();
        true
    }
}
