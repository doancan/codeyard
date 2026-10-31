//! The Search view in the accessibility tree: a `Group` with the query (`SearchInput`), its options (`CheckBox`), the
//! replacement, Replace All, the include/exclude fields behind their disclosure button, the status line (a polite
//! `Status`; a search or pattern error is an assertive `Alert`) and the results as a `Tree` of files (expandable, with
//! Replace in this file and Dismiss) and their matches. The model and the actions are the view's own
//! (`search_access.rs`); a row is named by its file, line and column, so an outdated tree acts on nothing else.

use super::*;
use crate::search::{AccessField, AccessFrame as Frame, AccessKey, AccessView, DISMISS_FILE, REPLACE_IN_FILE, SearchOption};

const SEARCH: NodeId = NodeId(102);
const QUERY: NodeId = NodeId(103);
const REPLACE: NodeId = NodeId(104);
const INCLUDE: NodeId = NodeId(105);
const EXCLUDE: NodeId = NodeId(106);
const REPLACE_ALL: NodeId = NodeId(107);
const GLOBS: NodeId = NodeId(108);
const STATUS_LINE: NodeId = NodeId(109);
const ERROR: NodeId = NodeId(110);
const RESULTS: NodeId = NodeId(111);
/// The options, in `OPTIONS` order.
const OPTION: u64 = 112;
const OPTIONS: [SearchOption; 5] = [SearchOption::Case, SearchOption::Word, SearchOption::Regex, SearchOption::Preserve, SearchOption::Structural];
/// This module's fixed nodes, for the id layout check.
pub(in crate::workbench) const FIXED: [NodeId; 15] = [
    SEARCH,
    QUERY,
    REPLACE,
    INCLUDE,
    EXCLUDE,
    REPLACE_ALL,
    GLOBS,
    STATUS_LINE,
    ERROR,
    RESULTS,
    NodeId(OPTION),
    NodeId(OPTION + 1),
    NodeId(OPTION + 2),
    NodeId(OPTION + 3),
    NodeId(OPTION + 4),
];
/// Result rows: row `i` of the exposed range at `SEARCH_ROW + 4i`, a file's buttons just above it.
pub(in crate::workbench) const SEARCH_ROW: u64 = 0x120_0000;
const ROW_IDS: u64 = 4;
/// Rows exposed at most.
pub(super) const MAX_ROWS: usize = 500;

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct SearchSnap {
    pub(crate) view: AccessView,
}

fn option_label(o: SearchOption) -> &'static str {
    match o {
        SearchOption::Case => t("Match case"),
        SearchOption::Word => t("Whole word"),
        SearchOption::Regex => t("Regular expression"),
        SearchOption::Preserve => t("Preserve case when replacing"),
        SearchOption::Structural => t("Structural search"),
    }
}

fn option_node(o: SearchOption) -> NodeId {
    NodeId(OPTION + OPTIONS.iter().position(|x| *x == o).unwrap_or(0) as u64)
}

fn field_node(f: AccessField) -> NodeId {
    match f {
        AccessField::Query => QUERY,
        AccessField::Replace => REPLACE,
        AccessField::Include => INCLUDE,
        AccessField::Exclude => EXCLUDE,
    }
}

fn row_node(i: usize) -> NodeId {
    NodeId(SEARCH_ROW + i as u64 * ROW_IDS)
}

impl SearchSnap {
    fn place(&self, frame: &Frame, node: &mut Node) {
        if let Some(bounds) = self.view.frames.get(frame) {
            node.set_bounds(*bounds);
        }
    }

    fn field(&self, nodes: &mut Vec<(NodeId, Node)>, which: AccessField, role: Role, label: &str, value: &str) -> NodeId {
        let id = field_node(which);
        let mut n = Node::new(role);
        n.set_label(label.to_string());
        n.set_value(value.to_string());
        n.add_action(Action::Focus);
        n.add_action(Action::SetValue);
        self.place(&Frame::Field(which), &mut n);
        nodes.push((id, n));
        id
    }

    fn button(&self, nodes: &mut Vec<(NodeId, Node)>, id: NodeId, label: &str, enabled: bool, frame: Frame) -> NodeId {
        let mut n = Node::new(Role::Button);
        n.set_label(label.to_string());
        if enabled {
            n.add_action(Action::Click);
        } else {
            n.set_disabled();
        }
        self.place(&frame, &mut n);
        nodes.push((id, n));
        id
    }

    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
        let v = &self.view;
        let mut kids = Vec::new();
        let query_label = if v.structural { t("Structural pattern") } else { t("Search") };
        kids.push(self.field(nodes, AccessField::Query, Role::SearchInput, query_label, &v.query));
        for (o, on) in &v.options {
            let id = option_node(*o);
            let mut n = Node::new(Role::CheckBox);
            n.set_label(option_label(*o).to_string());
            n.set_toggled(Toggled::from(*on));
            n.add_action(Action::Click);
            self.place(&Frame::Option(*o), &mut n);
            nodes.push((id, n));
            kids.push(id);
        }
        kids.push(self.field(nodes, AccessField::Replace, Role::TextInput, t("Replace"), &v.replace));
        kids.push(self.button(nodes, REPLACE_ALL, t("Replace All"), v.can_replace, Frame::ReplaceAll));
        let mut globs = Node::new(Role::Button);
        globs.set_label(t("Files to include / exclude"));
        globs.set_expanded(v.globs.is_some());
        globs.add_action(Action::Click);
        self.place(&Frame::Globs, &mut globs);
        nodes.push((GLOBS, globs));
        kids.push(GLOBS);
        if let Some((include, exclude)) = &v.globs {
            kids.push(self.field(nodes, AccessField::Include, Role::TextInput, t("Files to include"), include));
            kids.push(self.field(nodes, AccessField::Exclude, Role::TextInput, t("Files to exclude"), exclude));
        }
        if let Some(status) = &v.status {
            let mut n = Node::new(Role::Status);
            n.set_label(status.clone());
            n.set_live(Live::Polite);
            self.place(&Frame::Status, &mut n);
            nodes.push((STATUS_LINE, n));
            kids.push(STATUS_LINE);
        }
        if let Some(error) = &v.error {
            // Interrupts speech: the query cannot run as written.
            let mut n = Node::new(Role::Alert);
            n.set_label(error.clone());
            n.set_live(Live::Assertive);
            n.set_live_atomic();
            self.place(&Frame::Status, &mut n);
            nodes.push((ERROR, n));
            kids.push(ERROR);
        }
        let mut tree = Node::new(Role::Tree);
        tree.set_label(t("Search results"));
        tree.set_size_of_set(v.total);
        set_scroll(&mut tree, v.first, v.rows.len(), v.total);
        let mut ids = Vec::new();
        for (i, row) in v.rows.iter().enumerate() {
            let id = row_node(i);
            let mut n = Node::new(Role::TreeItem);
            n.set_label(row.label.clone());
            n.set_position_in_set(v.first + i + 1);
            n.set_size_of_set(v.total);
            self.place(&Frame::Row(row.key.clone()), &mut n);
            match &row.key {
                AccessKey::File(path) => {
                    n.set_level(1);
                    n.set_value(tn("{} match", "{} matches", row.count, &[]));
                    if !row.detail.is_empty() {
                        n.set_description(row.detail.clone());
                    }
                    n.set_expanded(row.expanded);
                    n.add_action(Action::Click);
                    n.add_action(if row.expanded { Action::Collapse } else { Action::Expand });
                    let name = crate::ui::file_name(path).to_string();
                    let mut buttons = Vec::new();
                    for (k, label) in [(REPLACE_IN_FILE, t("Replace in this file")), (DISMISS_FILE, t("Dismiss this file from the results"))] {
                        let bid = NodeId(id.0 + u64::from(k));
                        self.button(nodes, bid, label, true, Frame::Button(row.key.clone(), k));
                        if let Some((_, b)) = nodes.last_mut() {
                            b.set_description(name.clone());
                        }
                        buttons.push(bid);
                    }
                    n.set_children(buttons);
                }
                AccessKey::Match(..) => {
                    n.set_level(2);
                    n.set_description(row.detail.clone());
                    n.set_selected(row.current);
                    n.add_action(Action::Click);
                }
            }
            nodes.push((id, n));
            ids.push(id);
        }
        tree.set_children(ids);
        nodes.push((RESULTS, tree));
        kids.push(RESULTS);
        let mut group = Node::new(Role::Group);
        group.set_label(t("Search"));
        group.set_children(kids);
        nodes.push((SEARCH, group));
        SEARCH
    }

    /// The field with the keyboard, if one of the view's has it.
    pub(super) fn focus_node(&self) -> Option<NodeId> {
        self.view.focus.map(field_node)
    }
}

impl Workbench {
    /// The Search view while it is shown (in a side bar or a window of its own, whose tree takes it from here).
    pub(super) fn a11y_search_view(&self, window: &Window, cx: &App) -> Option<SearchSnap> {
        self.view_shown(Panel::Search).then(|| SearchSnap { view: self.search.read(cx).access_view(MAX_ROWS, window, cx) })
    }

    /// An action on the Search view's nodes; `false` when the node is not one of them.
    pub(super) fn a11y_search_view_action(&mut self, req: &ActionRequest, last: Option<&SearchSnap>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        let is_row = (SEARCH_ROW..SEARCH_ROW + BLOCK).contains(&id.0);
        if !(is_row || FIXED.contains(&id)) {
            return false;
        }
        let Some(last) = last.filter(|_| self.view_shown(Panel::Search)) else { return true };
        let field = [AccessField::Query, AccessField::Replace, AccessField::Include, AccessField::Exclude].into_iter().find(|f| field_node(*f) == id);
        match (req.action, field) {
            (Action::Focus, Some(f)) => self.search.update(cx, |s, cx| s.a11y_focus_field(f, window, cx)),
            (Action::SetValue, Some(f)) => {
                if let Some(ActionData::Value(text)) = &req.data {
                    self.search.update(cx, |s, cx| s.a11y_set_field(f, text, cx));
                }
            }
            _ if id == REPLACE_ALL && req.action == Action::Click => {
                if self.search.read(cx).access_view(MAX_ROWS, window, cx).can_replace {
                    self.search.update(cx, |s, cx| s.request_replace_all(cx));
                }
            }
            _ if id == GLOBS && req.action == Action::Click => {
                let was = last.view.globs.is_some();
                self.search.update(cx, |s, cx| s.a11y_toggle_globs(was, cx));
            }
            _ if id == RESULTS && matches!(req.action, Action::ScrollUp | Action::ScrollDown) => {
                let pages = if req.action == Action::ScrollUp { -1 } else { 1 };
                self.search.update(cx, |s, cx| s.a11y_scroll(pages, cx));
            }
            _ if (OPTION..OPTION + OPTIONS.len() as u64).contains(&id.0) && req.action == Action::Click => {
                let o = OPTIONS[(id.0 - OPTION) as usize];
                if let Some((_, was)) = last.view.options.iter().find(|(x, _)| *x == o) {
                    let was = *was;
                    self.search.update(cx, |s, cx| s.a11y_toggle(o, was, cx));
                }
            }
            _ if is_row => {
                let offset = (id.0 - SEARCH_ROW) % ROW_IDS;
                let Some(row) = last.view.rows.get(((id.0 - SEARCH_ROW) / ROW_IDS) as usize) else { return true };
                match (&row.key, offset, req.action) {
                    (AccessKey::File(path), 0, Action::Click) => {
                        let open = !row.expanded;
                        self.search.update(cx, |s, cx| s.a11y_expand(path, open, cx));
                    }
                    (AccessKey::File(path), 0, Action::Expand | Action::Collapse) => {
                        let open = req.action == Action::Expand;
                        self.search.update(cx, |s, cx| s.a11y_expand(path, open, cx));
                    }
                    (AccessKey::File(path), k, Action::Click) if k > 0 => {
                        self.search.update(cx, |s, cx| s.a11y_file_button(path, k as u8, cx));
                    }
                    (AccessKey::Match(path, line, column), 0, Action::Click) => {
                        self.search.update(cx, |s, cx| s.a11y_open(path, *line, *column, cx));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        true
    }
}

#[cfg(test)]
#[path = "a11y_search_tests.rs"]
mod tests;
