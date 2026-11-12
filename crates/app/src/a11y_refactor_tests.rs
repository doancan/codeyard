//! The Refactor Preview through the accessibility tree only: the tree of files and changes with their check boxes, choosing
//! and folding from the tree, a stale action ignored, and what is announced when the refactoring is written.

use accesskit::{Action, ActionRequest, Live, Node, NodeId, Role, Toggled, TreeId};
use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::*;
use crate::qa_integration_tests::wait_ticking;

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

/// The node of this role and label; a tree item is one of the preview's (the Explorer has rows with the same names).
fn find(snap: &Snapshot, role: Role, label: &str) -> Option<(NodeId, Node)> {
    let ours = |id: &NodeId| role != Role::TreeItem || (REFACTOR_ROW..REFACTOR_ROW + BLOCK).contains(&id.0);
    snap.to_tree().nodes.into_iter().find(|(id, n)| n.role() == role && n.label() == Some(label) && ours(id))
}

fn node(snap: &Snapshot, role: Role, label: &str) -> (NodeId, Node) {
    find(snap, role, label).unwrap_or_else(|| panic!("{role:?} {label:?} is in the tree"))
}

fn act(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId, action: Action) {
    let req = ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

fn edit(path: &std::path::Path, edits: Vec<(u32, u32, u32)>) -> Value {
    let list: Vec<Value> = edits
        .into_iter()
        .map(|(l, a, b)| json!({ "range": { "start": { "line": l, "character": a }, "end": { "line": l, "character": b } }, "newText": "k" }))
        .collect();
    json!({ "textDocument": { "uri": format!("file://{}", crate::ui::local_path(path)), "version": null }, "edits": list })
}

#[gpui::test]
fn the_preview_is_a_tree_of_check_boxes_that_is_chosen_folded_and_applied_from_the_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = crate::ui::canonical_for_test(crate::qa_workbench_tests::fresh("a11y-refactor"));
    let (a, b) = (dir.join("a.rs"), dir.join("b.rs"));
    std::fs::write(&a, "fn a() { x(); x(); }\n").unwrap();
    std::fs::write(&b, "fn b() {  x(); }\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    let proposal = json!({ "documentChanges": [edit(&a, vec![(0, 9, 10), (0, 14, 15)]), edit(&b, vec![(0, 10, 11)])] });
    wb.update(cx, |wb, cx| wb.review_workspace_edit_for_test("Rename to “k”", &proposal, None, cx));
    wait_ticking(cx, "the page", |cx| wb.read_with(cx, |wb, _| wb.refactor_page_for_test().is_some()));

    let snap = snapshot(&wb, cx);
    // The page, its summary and its live status.
    let (_, page) = node(&snap, Role::Group, "Rename to “k”");
    assert_eq!(page.description(), Some("Proposed: 3 changes in 2 files."));
    let (_, status) = node(&snap, Role::Status, "Selected: 3 changes in 2 files.");
    assert_eq!((status.live(), status.is_live_atomic()), (Some(Live::Polite), true));
    // Files are level 1 with a state and fold; their changes are level 2 check boxes; the line after is text.
    let (file_a, fa) = node(&snap, Role::TreeItem, "a.rs");
    assert_eq!((fa.level(), fa.is_expanded(), fa.toggled(), fa.description()), (Some(1), Some(true), Some(Toggled::True), Some("2 of 2 changes")));
    assert!(fa.supports_action(Action::Collapse) && fa.supports_action(Action::Click));
    let change = "Line 1, column 10: “x” → “k”";
    let (first, c1) = node(&snap, Role::TreeItem, change);
    assert_eq!((c1.level(), c1.toggled()), (Some(2), Some(Toggled::True)));
    assert!(find(&snap, Role::Tree, "Refactor Preview").is_some());
    // Every part has the frame it is drawn at, and the tree is one AccessKit accepts.
    let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
    for (role, label) in [(Role::Tree, "Refactor Preview"), (Role::TreeItem, "a.rs"), (Role::Button, "Apply Selected"), (Role::Button, "Discard")] {
        assert!(node(&snap, role, label).1.bounds().is_some(), "{label} has a frame");
    }
    drop(tree);
    // The keyboard row is the one with the focus.
    wb.update_in(cx, |wb, window, cx| wb.refactor_page_for_test().unwrap().update(cx, |v, cx| v.focus_list(window, cx)));
    assert_eq!(snapshot(&wb, cx).to_tree().focus, file_a);

    // Clear one change: the file is partly chosen, and the status says so.
    act(&wb, cx, &snap, first, Action::Click);
    let snap2 = snapshot(&wb, cx);
    assert_eq!(node(&snap2, Role::TreeItem, change).1.toggled(), Some(Toggled::False));
    assert_eq!(node(&snap2, Role::TreeItem, "a.rs").1.toggled(), Some(Toggled::Mixed));
    assert!(find(&snap2, Role::Status, "Selected: 2 changes in 2 files.").is_some());
    // The same press from the old tree, which showed it chosen, does not choose it again.
    act(&wb, cx, &snap, first, Action::Click);
    assert_eq!(node(&snapshot(&wb, cx), Role::TreeItem, change).1.toggled(), Some(Toggled::False));

    // Fold and unfold a file from the tree.
    let snap3 = snapshot(&wb, cx);
    act(&wb, cx, &snap3, node(&snap3, Role::TreeItem, "b.rs").0, Action::Collapse);
    let snap4 = snapshot(&wb, cx);
    assert_eq!(node(&snap4, Role::TreeItem, "b.rs").1.is_expanded(), Some(false));
    assert!(find(&snap4, Role::TreeItem, "Line 1, column 11: “x” → “k”").is_none());
    act(&wb, cx, &snap4, node(&snap4, Role::TreeItem, "b.rs").0, Action::Expand);

    // Apply from the button; the report is announced and the tree gives way to Undo and Close.
    let snap5 = snapshot(&wb, cx);
    let (apply, button) = node(&snap5, Role::Button, "Apply Selected");
    assert!(button.supports_action(Action::Click) && !button.is_disabled());
    act(&wb, cx, &snap5, apply, Action::Click);
    wait_ticking(cx, "the report", |cx| find(&snapshot(&wb, cx), Role::Status, "Changed 2 changes in 2 files").is_some());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "fn a() { x(); k(); }\n");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "fn b() {  k(); }\n");
    let done = snapshot(&wb, cx);
    assert!(find(&done, Role::Tree, "Refactor Preview").is_none(), "nothing left to choose");
    assert!(find(&done, Role::Button, "Undo").is_some());
    let (close, _) = node(&done, Role::Button, "Close");
    // Undo from the tree takes the text back.
    act(&wb, cx, &done, node(&done, Role::Button, "Undo").0, Action::Click);
    wait_ticking(cx, "the undo", |_| std::fs::read_to_string(&a).unwrap() == "fn a() { x(); x(); }\n");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "fn b() {  x(); }\n");
    // Close.
    let done = snapshot(&wb, cx);
    act(&wb, cx, &done, close, Action::Click);
    assert!(snapshot(&wb, cx).refactor.is_none());
}
