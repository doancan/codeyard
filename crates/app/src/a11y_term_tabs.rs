//! Terminal tabs of the focused editor group in the tab list. A click brings the terminal to the front the way the
//! pointer does; it is matched by entity, so a tab that closed or moved since the snapshot changes nothing.

use super::*;

/// Terminal tabs (`TERM_TAB + i`).
pub(super) const TERM_TAB: u64 = 0x1f0_0000;

/// The terminal in front for screen readers: its text as runs, its cursor and "Terminal 2 of 3".
pub(super) fn terminal_snap(term: &Entity<crate::terminal::TerminalView>, ix: usize, count: usize, cx: &App) -> TerminalSnap {
    let (rows, cursor) = term.read(cx).a11y_view();
    let mut starts = Vec::with_capacity(rows.len());
    let mut at = 0;
    for row in &rows {
        starts.push(at);
        at += row.chars().count() + 1;
    }
    let total = rows.len();
    let cursor = cursor.and_then(|(row, col)| starts.get(row).map(|s| s + col.min(rows[row].chars().count())));
    TerminalSnap { label: tf("Terminal {0} of {1}", &[&(ix + 1), &count]), runs: runs_for(starts.into_iter().zip(rows).collect(), total, 0), cursor }
}

/// A terminal tab of the focused group.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct TermTabSnap {
    pub(super) entity: gpui::EntityId,
    pub(crate) title: String,
    pub(crate) active: bool,
}

/// Append the terminal tabs after the `before` tabs of a list of `total`.
pub(super) fn append_tabs(tabs: &[TermTabSnap], before: usize, total: usize, ids: &mut Vec<NodeId>, nodes: &mut Vec<(NodeId, Node)>) {
    for (i, s) in tabs.iter().enumerate() {
        let id = NodeId(TERM_TAB + i as u64);
        let mut tab = Node::new(Role::Tab);
        tab.set_label(tf("{} (terminal)", &[&s.title]));
        tab.set_selected(s.active);
        tab.set_position_in_set(before + i + 1);
        tab.set_size_of_set(total);
        tab.add_action(Action::Click);
        tab.add_action(Action::Focus);
        ids.push(id);
        nodes.push((id, tab));
    }
}

impl Workbench {
    pub(super) fn a11y_term_tabs(&self, cx: &App) -> Vec<TermTabSnap> {
        let front = self.front_editor_terminal();
        self.tabs
            .terminals
            .iter()
            .map(|t| TermTabSnap { entity: t.entity_id(), title: t.read(cx).title.to_string(), active: front.as_ref() == Some(t) })
            .collect()
    }

    /// Handle an action aimed at a terminal tab; `false` if it is not one.
    pub(super) fn a11y_term_tab_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        if !(TERM_TAB..TERM_TAB + BLOCK).contains(&id.0) {
            return false;
        }
        let Some(tab) = last.term_tabs.get((id.0 - TERM_TAB) as usize) else { return true };
        let key = self.tabs.current_key();
        if let Some(term) = self.tabs.terminals.iter().find(|t| t.entity_id() == tab.entity).cloned()
            && self.ensure_group(key)
        {
            self.show_editor_terminal(term, window, cx);
        }
        true
    }
}

#[cfg(test)]
#[path = "a11y_term_tabs_tests.rs"]
mod tests;
