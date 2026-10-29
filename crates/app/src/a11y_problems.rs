//! The Problems panel for assistive technology: each problem with its severity, file and position, the "no problems"
//! state, and opening a problem by activating it. The list is bounded; an activation from an outdated tree opens nothing
//! (the row must still be the same problem at the same place).

use super::*;
use crate::workbench::panels_glue::ProblemRow;

#[path = "a11y_pages.rs"]
mod pages;
pub(super) use pages::{FIXED as PAGE_FIXED, PAGE_ROW};
#[path = "a11y_side_views.rs"]
mod side;
pub(super) use side::{DEBUG_FIXED, DEBUG_ROW, FIXED as SIDE_FIXED, MENU_ITEM, REMOTE_ROW, TEST_ROW};

const PROBLEMS: NodeId = NodeId(68);
const PROBLEMS_EMPTY: NodeId = NodeId(69);
pub(super) const PROBLEM_ROW: u64 = 0x170_0000;
/// Rows exposed at most (the panel can hold thousands; the count says how many there are).
const MAX_ROWS: usize = 2_000;

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ProblemsSnap {
    rows: Vec<ProblemRow>,
    problems: usize,
}

fn severity(s: u8) -> String {
    match s {
        1 => t("Error").into(),
        2 => t("Warning").into(),
        _ => t("Information").into(),
    }
}

impl ProblemsSnap {
    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
        if self.rows.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label(t("No problems."));
            empty.set_live(Live::Polite);
            nodes.push((PROBLEMS_EMPTY, empty));
            return PROBLEMS_EMPTY;
        }
        let mut list = Node::new(Role::List);
        list.set_label(tf("Problems: {}", &[&self.problems]));
        let mut kids = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            let id = NodeId(PROBLEM_ROW + i as u64);
            let mut n = Node::new(Role::ListItem);
            match row {
                ProblemRow::File { path, count, .. } => {
                    n.set_role(Role::Heading);
                    n.set_label(tf("{0}, problems: {1}", &[&crate::ui::file_name(path), count]));
                }
                ProblemRow::Problem { path, severity: s, text, line, col, .. } => {
                    n.set_label(format!("{}: {text}", severity(*s)));
                    n.set_description(tf("{0}, line {1}, column {2}", &[&crate::ui::file_name(path), &(line + 1), &(col + 1)]));
                    n.add_action(Action::Click);
                }
                ProblemRow::Related { text, path, line, .. } => {
                    n.set_label(text.clone());
                    n.set_description(tf("{0}, line {1}", &[&crate::ui::file_name(path), &(line + 1)]));
                    n.add_action(Action::Click);
                }
            }
            nodes.push((id, n));
            kids.push(id);
        }
        list.set_children(kids);
        nodes.push((PROBLEMS, list));
        PROBLEMS
    }
}

impl Workbench {
    /// The panel's rows while the Problems panel is open.
    pub(super) fn a11y_problems(&self, cx: &App) -> Option<ProblemsSnap> {
        if !(self.chrome.panel_open && self.chrome.bottom == Bottom::Problems) {
            return None;
        }
        let all = self.problems_shown();
        let mut rows = crate::workbench::panels_glue::problem_rows(&all, self.root(cx).as_deref());
        let problems = rows.iter().filter(|r| matches!(r, ProblemRow::Problem { .. })).count();
        rows.truncate(MAX_ROWS);
        Some(ProblemsSnap { rows, problems })
    }

    /// An activation of a row: opened only if the panel still has that very row there.
    pub(super) fn a11y_problems_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node.0;
        if !(PROBLEM_ROW..PROBLEM_ROW + BLOCK).contains(&id) {
            return false;
        }
        let index = (id - PROBLEM_ROW) as usize;
        let (Some(old), Some(current)) = (last.views.problems.as_ref(), self.a11y_problems(cx)) else { return true };
        if req.action == Action::Click && old.rows.get(index).is_some_and(|row| current.rows.get(index) == Some(row)) {
            self.open_problem_row(index, window, cx);
        }
        true
    }
}

/// The bottom panel's tabs and, while it is shown, the Output text.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct PanelSnap {
    /// (tab, label, count) in screen order.
    tabs: Vec<(Bottom, String, usize)>,
    selected: Bottom,
    output: Option<String>,
}

const PANEL_TABS: NodeId = NodeId(50);
const OUTPUT_LOG: NodeId = NodeId(51);
const PANEL_TAB: u64 = PROBLEM_ROW + BLOCK / 2;
/// Output exposed at most (the panel shows the last 400 lines).
const MAX_OUTPUT_LINES: usize = 400;

impl PanelSnap {
    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>) -> Vec<NodeId> {
        let mut list = Node::new(Role::TabList);
        list.set_label(t("Bottom panel"));
        let mut kids = Vec::new();
        for (i, (tab, label, count)) in self.tabs.iter().enumerate() {
            let id = NodeId(PANEL_TAB + i as u64);
            let mut n = Node::new(Role::Tab);
            n.set_label(label.clone());
            if *count > 0 {
                n.set_description(count.to_string());
            }
            n.set_selected(*tab == self.selected);
            n.add_action(Action::Click);
            nodes.push((id, n));
            kids.push(id);
        }
        list.set_children(kids);
        nodes.push((PANEL_TABS, list));
        let mut out = vec![PANEL_TABS];
        if let Some(text) = &self.output {
            let mut log = Node::new(Role::Log);
            log.set_label(t("Output"));
            log.set_value(if text.is_empty() { t("No output yet. Language server logs appear here.").to_string() } else { text.clone() });
            log.set_live(Live::Polite);
            nodes.push((OUTPUT_LOG, log));
            out.push(OUTPUT_LOG);
        }
        out
    }
}

impl Workbench {
    pub(super) fn a11y_panel(&self) -> Option<PanelSnap> {
        if !self.chrome.panel_open {
            return None;
        }
        let (errors, warnings) = self.problem_counts();
        let mut tabs = vec![(Bottom::Problems, t("Problems").to_string(), errors + warnings), (Bottom::Output, t("Output").to_string(), 0)];
        if self.dbg.has_sessions() {
            tabs.push((Bottom::Debug, t("Debug Console").to_string(), 0));
        }
        tabs.push((Bottom::Ports, t("Ports").to_string(), self.active_forwards()));
        tabs.push((Bottom::Terminal, t("Terminal").to_string(), 0));
        let output = (self.chrome.bottom == Bottom::Output).then(|| {
            let skip = self.lsp.output.len().saturating_sub(MAX_OUTPUT_LINES);
            self.lsp.output.iter().skip(skip).cloned().collect::<Vec<_>>().join("\n")
        });
        Some(PanelSnap { tabs, selected: self.chrome.bottom, output })
    }

    /// A tab of the panel: switched to only if the panel still offers it.
    pub(super) fn a11y_panel_action(&mut self, req: &ActionRequest, last: &Snapshot, cx: &mut Context<Self>) -> bool {
        let id = req.target_node.0;
        if !(PANEL_TAB..PANEL_TAB + 16).contains(&id) {
            return false;
        }
        let index = (id - PANEL_TAB) as usize;
        let (Some(old), Some(current)) = (last.views.panel.as_ref(), self.a11y_panel()) else { return true };
        if req.action == Action::Click
            && let Some((tab, ..)) = old.tabs.get(index)
            && current.tabs.iter().any(|(t, ..)| t == tab)
        {
            self.chrome.bottom = *tab;
            cx.notify();
        }
        true
    }
}

/// The search side panel: status, then its rows (at most `MAX_SEARCH_ROWS`).
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct SearchSideSnap {
    status: String,
    rows: Vec<(String, Option<(String, u32, u32)>)>,
    total: usize,
}

const SEARCH_SIDE: NodeId = NodeId(52);
const SEARCH_SIDE_STATUS: NodeId = NodeId(53);
const SEARCH_SIDE_ROW: u64 = PANEL_TAB + 16;
/// This module's fixed nodes, for the id layout check.
pub(super) const FIXED: [NodeId; 6] = [PROBLEMS, PROBLEMS_EMPTY, PANEL_TABS, OUTPUT_LOG, SEARCH_SIDE, SEARCH_SIDE_STATUS];
const MAX_SEARCH_ROWS: usize = 500;

/// The views exposed by this module, gathered so the main tree has one field and one action path for them.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct ViewsSnap {
    pub(crate) problems: Option<ProblemsSnap>,
    pub(crate) panel: Option<PanelSnap>,
    pub(crate) search: Option<SearchSideSnap>,
    pub(crate) pages: pages::PagesSnap,
    pub(crate) side: side::SideSnap,
}

impl ViewsSnap {
    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>, kids: &mut Vec<NodeId>) {
        if let Some(s) = &self.search {
            let mut status = Node::new(Role::Status);
            status.set_label(s.status.clone());
            status.set_live(Live::Polite);
            nodes.push((SEARCH_SIDE_STATUS, status));
            let mut list = Node::new(Role::List);
            list.set_label(t("Search results"));
            list.set_size_of_set(s.total);
            let mut ids = vec![SEARCH_SIDE_STATUS];
            for (i, (label, key)) in s.rows.iter().enumerate() {
                let id = NodeId(SEARCH_SIDE_ROW + i as u64);
                let mut n = Node::new(if key.is_some() { Role::ListItem } else { Role::Heading });
                n.set_label(label.clone());
                n.set_position_in_set(i + 1);
                n.set_size_of_set(s.total);
                if key.is_some() {
                    n.add_action(Action::Click);
                }
                nodes.push((id, n));
                ids.push(id);
            }
            list.set_children(ids);
            nodes.push((SEARCH_SIDE, list));
            kids.push(SEARCH_SIDE);
        }
        if let Some(panel) = &self.panel {
            kids.extend(panel.append_tree(nodes));
        }
        if let Some(problems) = &self.problems {
            kids.push(problems.append_tree(nodes));
        }
        self.pages.append_tree(nodes, kids);
        self.side.append_tree(nodes, kids);
    }

    /// The focused field of these views, if one has focus.
    pub(super) fn focus_node(&self) -> Option<NodeId> {
        self.side.focus_node().or_else(|| self.pages.focus_node())
    }
}

impl Workbench {
    pub(super) fn a11y_views(&self, window: &Window, cx: &App) -> ViewsSnap {
        let search = (self.chrome.panel == Some(Panel::Search)).then(|| {
            let (status, rows, total) = self.search.read(cx).access_rows(MAX_SEARCH_ROWS);
            SearchSideSnap { status, rows, total }
        });
        ViewsSnap { problems: self.a11y_problems(cx), panel: self.a11y_panel(), search, pages: self.a11y_pages(window, cx), side: self.a11y_side(cx) }
    }

    pub(super) fn a11y_views_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node.0;
        if (SEARCH_SIDE_ROW..SEARCH_SIDE_ROW + MAX_SEARCH_ROWS as u64).contains(&id) {
            let key = last.views.search.as_ref().and_then(|s| s.rows.get((id - SEARCH_SIDE_ROW) as usize)).and_then(|(_, k)| k.clone());
            if let (Action::Click, Some(key)) = (req.action, key) {
                self.search.update(cx, |s, cx| s.open_access_row(&key, cx));
            }
            return true;
        }
        self.a11y_panel_action(req, last, cx)
            || self.a11y_problems_action(req, last, window, cx)
            || self.a11y_pages_action(req, &last.views.pages, window, cx)
            || self.a11y_side_action(req, &last.views.side, window, cx)
    }
}

#[cfg(test)]
mod tests {
    use accesskit::{Action, ActionRequest, Role, TreeId};

    use super::*;

    fn diagnostic(line: u32, severity: u8, message: &str) -> crate::lsp::Diagnostic {
        crate::lsp::Diagnostic { start: (line, 2), end: (line, 4), severity, message: message.into(), ..Default::default() }
    }

    #[gpui::test]
    fn problems_are_listed_and_opened_and_an_outdated_row_opens_nothing(cx: &mut gpui::TestAppContext) {
        let dir = crate::qa_workbench_tests::project("a11y-problems");
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    let a = 1;\n    let b: u8 = \"x\";\n}\n").unwrap();
        let file = dir.join("src/main.rs").to_string_lossy().into_owned();
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        crate::qa_workbench_tests::open(&wb, &dir, cx);
        wb.update(cx, |wb, cx| {
            wb.chrome.panel_open = true;
            wb.chrome.bottom = Bottom::Problems;
            cx.notify();
        });
        // Empty: the state is announced, not an empty list.
        let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
        let nodes = snap.to_tree().nodes;
        assert!(nodes.iter().any(|(_, n)| n.role() == Role::Status && n.label() == Some("No problems.")));

        wb.update(cx, |wb, cx| {
            wb.lsp.diagnostics.insert(file.clone(), vec![diagnostic(0, 2, "unused"), diagnostic(2, 1, "mismatched types")]);
            cx.notify();
        });
        let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
        let nodes = snap.to_tree().nodes;
        assert!(nodes.iter().any(|(_, n)| n.role() == Role::List && n.label() == Some("Problems: 2")));
        let (error, node) = nodes.iter().find(|(_, n)| n.label() == Some("Error: mismatched types")).expect("the error row");
        assert_eq!(node.description(), Some("main.rs, line 3, column 3"), "position in one-based terms");
        assert!(node.supports_action(Action::Click));
        let req = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: *error, data: None };

        // The list changed since the tree was read: the same row index is another problem now, so nothing opens.
        wb.update(cx, |wb, cx| {
            wb.lsp.diagnostics.insert(file.clone(), vec![diagnostic(0, 1, "a new error"), diagnostic(0, 2, "unused"), diagnostic(2, 1, "mismatched types")]);
            cx.notify();
        });
        wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| wb.active_editor().is_none()), "an outdated activation does not open a file");

        // A fresh tree: activating the row opens the file at the problem.
        let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
        let nodes = snap.to_tree().nodes;
        let (error, _) = nodes.iter().find(|(_, n)| n.label() == Some("Error: mismatched types")).unwrap();
        let req = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: *error, data: None };
        wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
        crate::qa_integration_tests::wait_ticking(cx, "the file to open", |cx| wb.read_with(cx, |wb, _| wb.active_editor().is_some()));
        let (path, line) = wb.read_with(cx, |wb, cx| {
            let e = wb.active_editor().unwrap().read(cx);
            (e.path.clone(), e.cursor_position().0)
        });
        assert_eq!((path.as_str(), line), (file.as_str(), 3));
    }

    #[gpui::test]
    fn the_panel_tabs_switch_and_the_output_is_readable(cx: &mut gpui::TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update(cx, |wb, cx| {
            wb.chrome.panel_open = true;
            wb.chrome.bottom = Bottom::Problems;
            wb.log_output("rust-analyzer: indexing done".into());
            cx.notify();
        });
        let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
        let nodes = snap.to_tree().nodes;
        assert!(nodes.iter().any(|(_, n)| n.role() == Role::Tab && n.label() == Some("Problems") && n.is_selected() == Some(true)));
        assert!(!nodes.iter().any(|(_, n)| n.role() == Role::Log), "the Output text only while its tab is shown");
        let (output, _) = nodes.iter().find(|(_, n)| n.role() == Role::Tab && n.label() == Some("Output")).expect("Output tab");
        let req = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: *output, data: None };
        wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
        assert_eq!(wb.read_with(cx, |wb, _| wb.chrome.bottom), Bottom::Output);
        let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
        let nodes = snap.to_tree().nodes;
        let (_, log) = nodes.iter().find(|(_, n)| n.role() == Role::Log).expect("the Output text");
        assert_eq!(log.value(), Some("rust-analyzer: indexing done"));
        assert!(nodes.iter().any(|(_, n)| n.role() == Role::Tab && n.label() == Some("Output") && n.is_selected() == Some(true)));
    }

    #[gpui::test]
    fn search_results_in_the_side_panel_are_listed_and_open_their_match(cx: &mut gpui::TestAppContext) {
        let dir = crate::qa_workbench_tests::project("a11y-search-side");
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    let needle = 1;\n}\n").unwrap();
        let file = dir.join("src/main.rs").to_string_lossy().into_owned();
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        crate::qa_workbench_tests::open(&wb, &dir, cx);
        wb.update_in(cx, |wb, window, cx| wb.search_for("needle", window, cx));
        cx.simulate_keystrokes("enter");
        crate::qa_integration_tests::wait_ticking(cx, "the search to finish", |cx| {
            wb.read_with(cx, |wb, cx| wb.search_summary_for_test(cx)).is_some_and(|s| s.0 == 1)
        });
        let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
        let nodes = snap.to_tree().nodes;
        let statuses: Vec<_> = nodes.iter().filter(|(_, n)| n.role() == Role::Status).map(|(_, n)| n.label().map(str::to_string)).collect();
        // The same words as the panel's own status line.
        assert!(statuses.iter().any(|l| l.as_deref().is_some_and(|l| l.starts_with("1 result in "))), "status is announced: {statuses:?}");
        let (row, _) = nodes.iter().find(|(_, n)| n.role() == Role::ListItem && n.label() == Some("2: let needle = 1;")).expect("the match row");
        let req = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: *row, data: None };
        wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
        crate::qa_integration_tests::wait_ticking(cx, "the match to open", |cx| wb.read_with(cx, |wb, _| wb.active_editor().is_some()));
        let (path, line) = wb.read_with(cx, |wb, cx| {
            let e = wb.active_editor().unwrap().read(cx);
            (e.path.clone(), e.cursor_position().0)
        });
        assert_eq!((path.as_str(), line), (file.as_str(), 2));
    }
}
