//! The Problems panel's filter bar and rows through the accessibility tree: every control has a role and a name, the
//! filtered counts are announced, a file row folds, and a problem is copied.

use accesskit::{Action, ActionRequest, Live, Node, NodeId, Role, Toggled, TreeId};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::lsp::Diagnostic;

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn find(snap: &Snapshot, role: Role, label: &str) -> Option<(NodeId, Node)> {
    snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == role && n.label() == Some(label))
}

fn press(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId) {
    let req = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

fn diag(severity: u8, line: u32, message: &str) -> Diagnostic {
    Diagnostic { start: (line, 0), end: (line, 1), severity, message: message.into(), ..Default::default() }
}

fn fixture(cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = crate::qa_workbench_tests::project("a11y-problems-filter");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    let main = ui::local_path(&dir.join("src/main.rs"));
    wb.update_in(cx, |wb, window, cx| {
        wb.lsp.diagnostics.insert(main, vec![diag(1, 0, "cannot find value"), diag(2, 3, "unused variable"), diag(3, 5, "a note")]);
        wb.show_problems(&ShowProblems, window, cx);
    });
    cx.run_until_parked();
    (wb, cx)
}

#[gpui::test]
fn the_filter_bar_controls_are_named_and_the_filtered_counts_are_announced(cx: &mut TestAppContext) {
    let (wb, cx) = fixture(cx);
    let snap = snapshot(&wb, cx);
    for (role, label) in [
        (Role::Button, "Show Errors"),
        (Role::Button, "Show Warnings"),
        (Role::Button, "Show Information"),
        (Role::TextInput, "Filter problems"),
        (Role::Button, "Active File Only"),
        (Role::Button, "Sort: File"),
        (Role::Button, "Collapse All Files"),
        (Role::Button, "Copy All Problems"),
    ] {
        assert!(find(&snap, role, label).is_some(), "{role:?} {label:?} is in the tree");
    }
    let (_, errors) = find(&snap, Role::Button, "Show Errors").unwrap();
    assert_eq!(errors.toggled(), Some(Toggled::True), "a switch that is on says so");
    assert_eq!(errors.description(), Some("1 problems"));
    assert!(find(&snap, Role::Status, "1 errors, 1 warnings, 1 information").is_some_and(|(_, n)| n.live() == Some(Live::Polite)));

    // Turning warnings off from the tree: the switch says off, and the count says what is listed and of how many.
    let (warnings, _) = find(&snap, Role::Button, "Show Warnings").unwrap();
    press(&wb, cx, &snap, warnings);
    let snap = snapshot(&wb, cx);
    assert_eq!(find(&snap, Role::Button, "Show Warnings").unwrap().1.toggled(), Some(Toggled::False));
    assert!(find(&snap, Role::Status, "Showing 2 of 3 problems: 1 errors, 0 warnings, 1 information").is_some_and(|(_, n)| n.live() == Some(Live::Polite)));
    assert!(find(&snap, Role::ListItem, "Error: cannot find value").is_some());
    assert!(find(&snap, Role::ListItem, "Warning: unused variable").is_none(), "a hidden problem is not in the list");

    // Everything hidden: the empty state names the filter, not "no problems".
    for label in ["Show Errors", "Show Information"] {
        let snap = snapshot(&wb, cx);
        press(&wb, cx, &snap, find(&snap, Role::Button, label).unwrap().0);
    }
    let snap = snapshot(&wb, cx);
    assert!(find(&snap, Role::Status, "No problems match the filter.").is_some_and(|(_, n)| n.live() == Some(Live::Polite)));
}

#[gpui::test]
fn a_file_heading_folds_from_the_tree_and_a_problem_is_copied(cx: &mut TestAppContext) {
    let (wb, cx) = fixture(cx);
    let snap = snapshot(&wb, cx);
    let (file, heading) = find(&snap, Role::Heading, "main.rs, problems: 3").expect("the file heading");
    assert_eq!(heading.is_expanded(), Some(true));
    let (_, row) = find(&snap, Role::ListItem, "Warning: unused variable").unwrap();
    let copy =
        *row.children().iter().find(|id| snap.to_tree().nodes.iter().any(|(i, n)| i == *id && n.label() == Some("Copy Problem"))).expect("its copy button");
    press(&wb, cx, &snap, copy);
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("src/main.rs:4:1: warning unused variable"));

    press(&wb, cx, &snap, file);
    let snap = snapshot(&wb, cx);
    assert_eq!(find(&snap, Role::Heading, "main.rs, problems: 3").unwrap().1.is_expanded(), Some(false));
    assert!(find(&snap, Role::ListItem, "Warning: unused variable").is_none(), "a folded file lists no problems");
}
