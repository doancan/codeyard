//! Large explorers and files: the accessibility tree shows a bounded range around the selection or caret, follows
//! scroll actions to the visible area and keeps positions and offsets relative to the whole tree or document.

use accesskit::{Action, ActionRequest, Role, TreeId};
use gpui::TestAppContext;

use crate::qa_integration_tests::wait_ticking;
use crate::qa_workbench_tests::{fresh, open, setup};

fn scroll(action: Action, node: accesskit::NodeId) -> ActionRequest {
    ActionRequest { action, target_tree: TreeId::ROOT, target_node: node, data: None }
}

#[gpui::test]
fn the_explorer_range_follows_the_selection_and_scroll_actions(cx: &mut TestAppContext) {
    let dir = fresh("a11y-range-explorer");
    for i in 0..3_000 {
        std::fs::write(dir.join(format!("f{i:04}.txt")), "").unwrap();
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let tree = wb.read_with(cx, |wb, _| wb.roots.primary.clone());
    wait_ticking(cx, "rows", |cx| tree.read_with(cx, |t, _| t.row_names().len() == 3_000));
    let path = dir.join("f2500.txt").to_string_lossy().into_owned();
    tree.update(cx, |t, cx| t.a11y_select(&path, false, cx));
    cx.run_until_parked();
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let ex = &snap.explorers[0];
    assert_eq!((ex.rows_total, ex.rows.len()), (3_000, 1_000), "bounded");
    let ix = ex.rows.iter().position(|r| r.path == path).expect("the selected row is in the range");
    assert_eq!(ex.focused, Some(ex.rows[ix].id));
    assert!((400..=600).contains(&ix) || ex.first + ex.rows.len() == 3_000, "centred: {ix} from {}", ex.first);
    let nodes = snap.to_tree().nodes;
    let (id, tree_node) = nodes.iter().find(|(_, n)| n.role() == Role::Tree).unwrap();
    assert_eq!(tree_node.scroll_y(), Some(ex.first as f64));
    assert!(tree_node.supports_action(Action::ScrollUp));
    let item = nodes.iter().find(|(nid, _)| nid.0 == ex.rows[ix].id).unwrap();
    assert_eq!(item.1.position_in_set(), Some(2_501), "position in the whole tree");

    // Paging up moves the view away from the selection: the range follows the visible rows.
    for _ in 0..30 {
        let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
        wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&scroll(Action::ScrollUp, *id), &snap, window, cx));
        cx.run_until_parked();
    }
    let top = tree.read_with(cx, |t, _| t.visible_rows()).unwrap().0;
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    assert!(top < 2_000 && snap.explorers[0].first <= top, "visible from {top}, range from {}", snap.explorers[0].first);
    assert_eq!(snap.explorers[0].focused, None, "the selection is outside the range");
}

#[gpui::test]
fn the_editor_range_follows_the_caret_and_scroll_actions(cx: &mut TestAppContext) {
    let dir = fresh("a11y-range-editor");
    let file = dir.join("long.txt");
    std::fs::write(&file, (0..5_000).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor().cloned()).unwrap();
    let at: usize = (0..4_000).map(|i| format!("line {i}\n").len()).sum();
    ed.update_in(cx, |e, window, cx| {
        e.reveal(4_001, 0, 0, window, cx);
    });
    cx.run_until_parked();
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let e = snap.editor.as_ref().unwrap();
    assert!((3_800..=4_000).contains(&e.first_line), "centred on the caret: {}", e.first_line);
    let run = e.runs.iter().find(|r| r.text == "line 4000\n").expect("the caret line is exposed");
    assert_eq!(run.start, at, "offsets are document-relative");
    assert_eq!(e.selection, (at, at));
    let nodes = snap.to_tree().nodes;
    let (id, node) = nodes.iter().find(|(_, n)| n.role() == Role::MultilineTextInput).unwrap();
    assert_eq!(node.scroll_y(), Some(e.first_line as f64));
    assert_eq!(node.description(), Some(format!("5001 lines, showing lines {}–{}", e.first_line + 1, e.first_line + 300).as_str()));

    for _ in 0..20 {
        let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
        wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&scroll(Action::ScrollDown, *id), &snap, window, cx));
        cx.run_until_parked();
    }
    let (top, _) = ed.read_with(cx, |e, _| e.visible_lines()).unwrap();
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let e = snap.editor.as_ref().unwrap();
    assert!(top > 4_200 && e.first_line > 4_000, "visible from {top}, range from {}", e.first_line);
    assert_eq!(ed.read_with(cx, |e, _| e.a11y_view(0, 0).0), (at, at), "the caret stays");
}
