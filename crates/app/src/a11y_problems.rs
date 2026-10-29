//! The Problems panel for assistive technology: each problem with its severity, file and position, the "no problems"
//! state, and opening a problem by activating it. The list is bounded; an activation from an outdated tree opens nothing
//! (the row must still be the same problem at the same place).

use super::*;
use crate::workbench::panels_glue::ProblemRow;

pub(super) const PROBLEMS: NodeId = NodeId(68);
pub(super) const PROBLEMS_EMPTY: NodeId = NodeId(69);
pub(super) const PROBLEM_ROW: u64 = 0x110_0000;
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
        let (Some(old), Some(current)) = (last.problems.as_ref(), self.a11y_problems(cx)) else { return true };
        if req.action == Action::Click && old.rows.get(index).is_some_and(|row| current.rows.get(index) == Some(row)) {
            self.open_problem_row(index, window, cx);
        }
        true
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
}
