//! Source Control's history for assistive technology: the Sync Changes / Publish Branch button, the Worktrees section
//! (header, Add worktree, Refresh, a list of checkouts that open as a click opens them), the Graph section (header, its
//! buttons with their counts and busy state, the active filters, the commit search field and a `List` of day headings
//! and commits, each commit with Open Changes and More Actions… and `ShowContextMenu`) and the selected commit's details
//! (message lines and its files, which open their diffs). Every node has its painted frame. An action names what it
//! acts on (a commit hash, a worktree path, a file and its commit, a button's label): from an outdated tree it does
//! nothing when that changed or went away.

use super::*;
use crate::scm::{AccessFrame, AccessGraphRow, AccessHistory, HistoryFrame};

const SCM_SYNC: NodeId = NodeId(119);
const WORKTREES: NodeId = NodeId(120);
const WORKTREES_HEADER: NodeId = NodeId(121);
const WORKTREES_ADD: NodeId = NodeId(122);
const WORKTREES_REFRESH: NodeId = NodeId(123);
const WORKTREES_LIST: NodeId = NodeId(124);
const WORKTREES_NOTE: NodeId = NodeId(125);
const GRAPH: NodeId = NodeId(126);
const GRAPH_HEADER: NodeId = NodeId(127);
const GRAPH_SEARCH: NodeId = NodeId(128);
const GRAPH_FETCH: NodeId = NodeId(129);
const GRAPH_PULL: NodeId = NodeId(130);
const GRAPH_PUSH: NodeId = NodeId(131);
const GRAPH_REFRESH: NodeId = NodeId(132);
const GRAPH_MORE: NodeId = NodeId(133);
const GRAPH_FILTER: u64 = 134; // three ids: 134, 135, 136
const GRAPH_QUERY: NodeId = NodeId(137);
const GRAPH_LIST: NodeId = NodeId(138);
const GRAPH_NOTE: NodeId = NodeId(139);
const DETAIL: NodeId = NodeId(140);
const DETAIL_HEADER: NodeId = NodeId(141);
const DETAIL_CLOSE: NodeId = NodeId(142);
const DETAIL_FILES: NodeId = NodeId(143);
const DETAIL_NOTE: NodeId = NodeId(144);
const DETAIL_SUBJECT: NodeId = NodeId(145);
const DETAIL_META: NodeId = NodeId(146);
/// This module's fixed nodes, for the id layout check.
pub(in crate::workbench) const FIXED: [NodeId; 28] = [
    SCM_SYNC,
    WORKTREES,
    WORKTREES_HEADER,
    WORKTREES_ADD,
    WORKTREES_REFRESH,
    WORKTREES_LIST,
    WORKTREES_NOTE,
    GRAPH,
    GRAPH_HEADER,
    GRAPH_SEARCH,
    GRAPH_FETCH,
    GRAPH_PULL,
    GRAPH_PUSH,
    GRAPH_REFRESH,
    GRAPH_MORE,
    NodeId(GRAPH_FILTER),
    NodeId(GRAPH_FILTER + 1),
    NodeId(GRAPH_FILTER + 2),
    GRAPH_QUERY,
    GRAPH_LIST,
    GRAPH_NOTE,
    DETAIL,
    DETAIL_HEADER,
    DETAIL_CLOSE,
    DETAIL_FILES,
    DETAIL_NOTE,
    DETAIL_SUBJECT,
    DETAIL_META,
];
/// Graph rows: row `i` of the exposed range at `HISTORY_ROW + 4i` (its Open Changes and More Actions… buttons above it).
pub(in crate::workbench) const HISTORY_ROW: u64 = 0x150_0000;
/// Worktree rows (`+ i`), lines of the selected commit's message (`+ 0x4_0000 + i`) and its files (`+ 0x8_0000 + i`).
pub(in crate::workbench) const HISTORY_ITEM: u64 = 0x200_0000;
const DETAIL_LINE: u64 = HISTORY_ITEM + 0x4_0000;
const DETAIL_FILE: u64 = HISTORY_ITEM + 0x8_0000;
const OPEN: u64 = 1;
const MORE: u64 = 2;
/// Message lines and files of a commit exposed at most.
const MAX_DETAIL: usize = 2_000;
const _: () = assert!(crate::scm::HISTORY_ROWS as u64 * 4 <= BLOCK && 0x8_0000 + MAX_DETAIL as u64 <= BLOCK);

fn place(view: &crate::scm::AccessView, part: HistoryFrame, node: &mut Node) {
    if let Some(b) = view.frames.get(&AccessFrame::History(part)) {
        node.set_bounds(*b);
    }
}

fn button(nodes: &mut Vec<(NodeId, Node)>, view: &crate::scm::AccessView, id: NodeId, label: &str, part: HistoryFrame, enabled: bool) -> NodeId {
    let mut b = Node::new(Role::Button);
    b.set_label(label.to_string());
    if enabled {
        b.add_action(Action::Click);
    } else {
        b.set_disabled();
    }
    place(view, part, &mut b);
    nodes.push((id, b));
    id
}

fn header(nodes: &mut Vec<(NodeId, Node)>, view: &crate::scm::AccessView, id: NodeId, label: String, open: bool, section: &'static str) -> NodeId {
    let mut h = Node::new(Role::Button);
    h.set_label(label);
    h.set_expanded(open);
    h.add_action(Action::Click);
    place(view, HistoryFrame::Header(section), &mut h);
    nodes.push((id, h));
    id
}

fn note(nodes: &mut Vec<(NodeId, Node)>, id: NodeId, (text, error): &(String, bool)) -> NodeId {
    let mut n = Node::new(if *error { Role::Alert } else { Role::Status });
    n.set_label(text.clone());
    n.set_live(if *error { Live::Assertive } else { Live::Polite });
    nodes.push((id, n));
    id
}

impl ScmSnap {
    /// The Sync button (after the commit button) and the history sections, in the order they are drawn.
    pub(super) fn append_history(&self, nodes: &mut Vec<(NodeId, Node)>, sync: &mut Vec<NodeId>, children: &mut Vec<NodeId>) {
        let view = &self.view;
        let h = &view.history;
        if let Some(label) = &h.sync {
            sync.push(button(nodes, view, SCM_SYNC, label, HistoryFrame::Sync, !h.syncing));
            if h.syncing
                && let Some((_, n)) = nodes.last_mut()
            {
                n.set_busy();
            }
        }
        children.push(self.append_worktrees(nodes));
        children.push(self.append_graph(nodes));
        if let Some(d) = &h.detail {
            let mut group = Node::new(Role::Group);
            group.set_label(d.title.clone());
            let mut kids = vec![header(nodes, view, DETAIL_HEADER, d.title.clone(), d.open, "details")];
            kids.push(button(nodes, view, DETAIL_CLOSE, t("Close details"), HistoryFrame::Button("details-close"), true));
            if d.open {
                for (i, line) in d.lines.iter().take(MAX_DETAIL).enumerate() {
                    let id = match i {
                        0 => DETAIL_SUBJECT,
                        _ if i + 2 == d.lines.len().min(MAX_DETAIL) => DETAIL_META,
                        _ => NodeId(DETAIL_LINE + i as u64),
                    };
                    let mut n = Node::new(if i == 0 { Role::Heading } else { Role::Label });
                    n.set_label(line.clone());
                    nodes.push((id, n));
                    kids.push(id);
                }
                if let Some(text) = &d.note {
                    kids.push(note(nodes, DETAIL_NOTE, text));
                }
                if !d.files.is_empty() {
                    let mut list = Node::new(Role::List);
                    list.set_label(tf("Files: {}", &[&d.files.len()]));
                    let mut ids = Vec::new();
                    for (i, (path, change)) in d.files.iter().take(MAX_DETAIL).enumerate() {
                        let id = NodeId(DETAIL_FILE + i as u64);
                        let mut f = Node::new(Role::Button);
                        f.set_label(path.clone());
                        f.set_description(change.clone());
                        f.set_position_in_set(i + 1);
                        f.set_size_of_set(d.files.len());
                        f.add_action(Action::Click);
                        place(view, HistoryFrame::DetailFile(i), &mut f);
                        nodes.push((id, f));
                        ids.push(id);
                    }
                    list.set_children(ids);
                    nodes.push((DETAIL_FILES, list));
                    kids.push(DETAIL_FILES);
                }
            }
            place(view, HistoryFrame::Section("details"), &mut group);
            group.set_children(kids);
            nodes.push((DETAIL, group));
            children.push(DETAIL);
        }
    }

    fn append_worktrees(&self, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
        let view = &self.view;
        let w = &view.history.worktrees;
        let mut group = Node::new(Role::Group);
        group.set_label(t("Worktrees"));
        let mut kids = vec![header(nodes, view, WORKTREES_HEADER, format!("{} ({})", t("Worktrees"), w.rows.len()), w.open, "worktrees")];
        kids.push(button(nodes, view, WORKTREES_ADD, t("Add worktree"), HistoryFrame::Button("worktree-add"), true));
        kids.push(button(nodes, view, WORKTREES_REFRESH, t("Refresh"), HistoryFrame::Button("worktree-refresh"), true));
        if w.open {
            if let Some(text) = &w.note {
                kids.push(note(nodes, WORKTREES_NOTE, text));
            }
            if !w.rows.is_empty() {
                let mut list = Node::new(Role::List);
                list.set_label(t("Worktrees"));
                let mut ids = Vec::new();
                for (i, row) in w.rows.iter().enumerate().take(MAX_DETAIL) {
                    let id = NodeId(HISTORY_ITEM + i as u64);
                    let mut n = Node::new(Role::ListItem);
                    n.set_label(row.title.clone());
                    n.set_description(row.detail.clone());
                    n.set_selected(row.current);
                    n.set_position_in_set(i + 1);
                    n.set_size_of_set(w.rows.len());
                    if row.available && !row.current {
                        n.add_action(Action::Click);
                    } else {
                        n.set_disabled();
                    }
                    place(view, HistoryFrame::Worktree(row.path.clone()), &mut n);
                    nodes.push((id, n));
                    ids.push(id);
                }
                list.set_children(ids);
                place(view, HistoryFrame::List("worktrees"), &mut list);
                nodes.push((WORKTREES_LIST, list));
                kids.push(WORKTREES_LIST);
            }
        }
        place(view, HistoryFrame::Section("worktrees"), &mut group);
        group.set_children(kids);
        nodes.push((WORKTREES, group));
        WORKTREES
    }

    fn append_graph(&self, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
        let view = &self.view;
        let g = &view.history.graph;
        let mut group = Node::new(Role::Group);
        group.set_label(t("Graph"));
        if g.busy {
            group.set_busy();
        }
        let title = match g.count {
            Some(n) => format!("{} ({n})", t("Graph")),
            None => t("Graph").to_string(),
        };
        let mut kids = vec![header(nodes, view, GRAPH_HEADER, title, g.open, "graph")];
        let search = button(nodes, view, GRAPH_SEARCH, t("Search commits"), HistoryFrame::Button("search"), true);
        if let Some((_, n)) = nodes.last_mut() {
            n.set_toggled(Toggled::from(g.search_open));
        }
        kids.push(search);
        if g.has_remote {
            kids.push(button(nodes, view, GRAPH_FETCH, t("Fetch from all remotes"), HistoryFrame::Button("fetch"), !g.busy));
            for (id, label, n, part) in [(GRAPH_PULL, t("Pull"), g.behind, "pull"), (GRAPH_PUSH, t("Push"), g.ahead, "push")] {
                kids.push(button(nodes, view, id, label, HistoryFrame::Button(part), !g.busy));
                if n > 0
                    && let Some((_, node)) = nodes.last_mut()
                {
                    node.set_description(tn("{} commit", "{} commits", n as usize, &[]));
                }
            }
        }
        kids.push(button(nodes, view, GRAPH_REFRESH, t("Reload history"), HistoryFrame::Button("refresh"), !g.busy));
        if g.busy
            && let Some((_, n)) = nodes.last_mut()
        {
            n.set_busy();
        }
        kids.push(button(nodes, view, GRAPH_MORE, t("More Actions…"), HistoryFrame::Button("more"), true));
        if let Some((_, n)) = nodes.last_mut() {
            n.set_has_popup(accesskit::HasPopup::Menu);
        }
        if g.open {
            for (which, label) in &g.filters {
                let id = NodeId(GRAPH_FILTER + u64::from(*which));
                kids.push(button(nodes, view, id, label, HistoryFrame::Filter(*which), true));
                if let Some((_, n)) = nodes.last_mut() {
                    n.set_description(t("Click to remove this filter"));
                }
            }
            if g.search_open {
                let mut q = Node::new(Role::SearchInput);
                q.set_label(t("Search commits"));
                q.set_value(g.query.clone());
                q.add_action(Action::Focus);
                q.add_action(Action::SetValue);
                q.set_controls(vec![GRAPH_LIST]);
                place(view, HistoryFrame::Query, &mut q);
                nodes.push((GRAPH_QUERY, q));
                kids.push(GRAPH_QUERY);
            }
            if let Some(text) = &g.note {
                kids.push(note(nodes, GRAPH_NOTE, text));
            } else {
                let mut list = Node::new(Role::List);
                list.set_label(t("Commits"));
                set_scroll(&mut list, g.first, g.rows.len(), g.total);
                let mut ids = Vec::new();
                for (i, row) in g.rows.iter().enumerate() {
                    let id = NodeId(HISTORY_ROW + 4 * i as u64);
                    let n = match row {
                        AccessGraphRow::Day(label, count) => {
                            let mut n = Node::new(Role::Heading);
                            n.set_label(label.clone());
                            n.set_description(tn("{} commit", "{} commits", *count, &[]));
                            if let Some(b) = view.frames.get(&AccessFrame::History(HistoryFrame::Row(format!("day:{label}")))) {
                                n.set_bounds(*b);
                            }
                            n
                        }
                        AccessGraphRow::Commit { hash, subject, detail, selected } => {
                            let mut n = Node::new(Role::ListItem);
                            n.set_label(subject.clone());
                            n.set_description(detail.clone());
                            n.set_selected(*selected);
                            n.set_position_in_set(g.first + i + 1);
                            n.set_size_of_set(g.total);
                            n.add_action(Action::Click);
                            n.add_action(Action::ShowContextMenu);
                            place(view, HistoryFrame::Row(hash.clone()), &mut n);
                            let mut open = Node::new(Role::Button);
                            open.set_label(t("Open Changes"));
                            open.set_description(subject.clone());
                            open.add_action(Action::Click);
                            let mut more = Node::new(Role::Button);
                            more.set_label(t("More Actions…"));
                            more.set_description(subject.clone());
                            more.set_has_popup(accesskit::HasPopup::Menu);
                            more.add_action(Action::Click);
                            nodes.push((NodeId(id.0 + OPEN), open));
                            nodes.push((NodeId(id.0 + MORE), more));
                            n.set_children(vec![NodeId(id.0 + OPEN), NodeId(id.0 + MORE)]);
                            n
                        }
                    };
                    nodes.push((id, n));
                    ids.push(id);
                }
                list.set_children(ids);
                place(view, HistoryFrame::List("graph"), &mut list);
                nodes.push((GRAPH_LIST, list));
                kids.push(GRAPH_LIST);
            }
        }
        place(view, HistoryFrame::Section("graph"), &mut group);
        group.set_children(kids);
        nodes.push((GRAPH, group));
        GRAPH
    }

    /// The focused node of the history sections: the commit search field while it has the keyboard.
    pub(in crate::workbench) fn history_focus(&self) -> Option<NodeId> {
        let g = &self.view.history.graph;
        (g.search_open && g.query_focused).then_some(GRAPH_QUERY)
    }
}

/// Whether `id` is one of the history's nodes.
pub(super) fn is_history(id: NodeId) -> bool {
    FIXED.contains(&id) || (HISTORY_ROW..HISTORY_ROW + BLOCK).contains(&id.0) || (HISTORY_ITEM..HISTORY_ITEM + BLOCK).contains(&id.0)
}

impl Workbench {
    /// An action on the history (the repository and branch were checked by the caller).
    pub(super) fn a11y_history_action(&mut self, req: &ActionRequest, was: &AccessHistory, window: &mut Window, cx: &mut Context<Self>) {
        let now = self.scm.read(cx).a11y_view(window, cx).history;
        let id = req.target_node;
        match (req.action, id) {
            (Action::Click, SCM_SYNC) => {
                if let Some(label) = was.sync.as_ref().filter(|l| now.sync.as_ref() == Some(*l)) {
                    let label = label.clone();
                    self.scm.update(cx, |s, cx| s.a11y_sync(&label, cx));
                }
            }
            (Action::Click, WORKTREES_HEADER) if was.worktrees.open == now.worktrees.open => {
                self.scm.update(cx, |s, cx| s.a11y_toggle_history_section("worktrees", cx));
            }
            (Action::Click, GRAPH_HEADER) if was.graph.open == now.graph.open => {
                self.scm.update(cx, |s, cx| s.a11y_toggle_history_section("graph", cx));
            }
            (Action::Click, DETAIL_HEADER) if was.detail.as_ref().map(|d| (&d.hash, d.open)) == now.detail.as_ref().map(|d| (&d.hash, d.open)) => {
                self.scm.update(cx, |s, cx| s.a11y_toggle_history_section("details", cx));
            }
            (Action::Click, WORKTREES_ADD) => self.scm.update(cx, |s, cx| s.a11y_worktree_button("worktree-add", cx)),
            (Action::Click, WORKTREES_REFRESH) => self.scm.update(cx, |s, cx| s.a11y_worktree_button("worktree-refresh", cx)),
            (Action::Click, GRAPH_SEARCH) if was.graph.search_open == now.graph.search_open => {
                self.scm.update(cx, |s, cx| s.a11y_graph_button("search", window, cx));
            }
            (Action::Click, GRAPH_REFRESH) => self.scm.update(cx, |s, cx| s.a11y_graph_button("refresh", window, cx)),
            (Action::Click, GRAPH_MORE) => self.scm.update(cx, |s, cx| s.a11y_graph_button("more", window, cx)),
            (Action::Click, DETAIL_CLOSE) if was.detail.as_ref().map(|d| &d.hash) == now.detail.as_ref().map(|d| &d.hash) => {
                self.scm.update(cx, |s, cx| s.a11y_graph_button("details-close", window, cx));
            }
            // Fetch, pull and push are the same workbench actions their buttons dispatch.
            (Action::Click, GRAPH_FETCH | GRAPH_PULL | GRAPH_PUSH) if now.graph.has_remote && !now.graph.busy => {
                let action: Box<dyn gpui::Action> = match id {
                    GRAPH_FETCH => Box::new(crate::workbench::GitFetchAll),
                    GRAPH_PULL => Box::new(crate::workbench::GitPull),
                    _ => Box::new(crate::workbench::GitPush),
                };
                window.dispatch_action(action, cx);
            }
            (Action::Click, id) if (GRAPH_FILTER..GRAPH_FILTER + 3).contains(&id.0) => {
                let which = (id.0 - GRAPH_FILTER) as u8;
                if now.graph.filters.iter().any(|(w, _)| *w == which) {
                    self.scm.update(cx, |s, cx| s.a11y_remove_filter(which, cx));
                }
            }
            (Action::Focus, GRAPH_QUERY) => self.scm.update(cx, |s, cx| s.focus_graph_query(window, cx)),
            (Action::SetValue, GRAPH_QUERY) => {
                if let Some(ActionData::Value(text)) = &req.data {
                    self.scm.update(cx, |s, cx| s.a11y_set_graph_query(text, cx));
                }
            }
            (action, id) if (HISTORY_ROW..HISTORY_ROW + BLOCK).contains(&id.0) => {
                let (i, part) = (((id.0 - HISTORY_ROW) / 4) as usize, (id.0 - HISTORY_ROW) % 4);
                let Some(AccessGraphRow::Commit { hash, .. }) = was.graph.rows.get(i) else { return };
                let what = match (action, part) {
                    (Action::Click, 0) => "select",
                    (Action::Click, OPEN) => "open",
                    (Action::ShowContextMenu, 0) | (Action::Click, MORE) => "menu",
                    _ => return,
                };
                let hash = hash.clone();
                self.scm.update(cx, |s, cx| s.a11y_commit(&hash, what, cx));
            }
            (Action::Click, id) if (DETAIL_FILE..DETAIL_FILE + MAX_DETAIL as u64).contains(&id.0) => {
                let i = (id.0 - DETAIL_FILE) as usize;
                let Some(d) = &was.detail else { return };
                let Some((path, _)) = d.files.get(i) else { return };
                let (hash, path) = (d.hash.clone(), path.clone());
                self.scm.update(cx, |s, cx| s.a11y_open_detail_file(&hash, i, &path, cx));
            }
            (Action::Click, id) if (HISTORY_ITEM..HISTORY_ITEM + MAX_DETAIL as u64).contains(&id.0) => {
                let Some(row) = was.worktrees.rows.get((id.0 - HISTORY_ITEM) as usize) else { return };
                if now.worktrees.rows.iter().any(|r| r.path == row.path && r.available && !r.current) {
                    let path = row.path.clone();
                    self.scm.update(cx, |s, cx| s.a11y_open_worktree(&path, cx));
                }
            }
            _ => {}
        }
    }
}
