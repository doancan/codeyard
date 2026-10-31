//! Frames of individual rows, controls and characters: each is where it was painted (× display scale × interface
//! scale), and AccessKit's own hit testing and text-range bounds land on it, in the main window and in an editor's own
//! window, at 100% and 200%.

use super::geometry::Part;
use super::*;
use accesskit::{Point, Rect};
use accesskit_consumer::{FilterResult, NodeRef, Tree};
use gpui::{Pixels, TestAppContext, VisualTestContext, size};

const TEXT: &str = "fn größe() {\n    let café = \"naïve\";\n}\n";

fn rig<'cx>(cx: &'cx mut TestAppContext, name: &str) -> (Entity<Workbench>, &'cx mut VisualTestContext, std::path::PathBuf) {
    let (wb, cx) = crate::qa_integration_tests::setup_with(cx, Settings { lsp_enabled: false, ..Default::default() });
    let project = crate::qa_workbench_tests::project(name);
    let file = project.join("src/text.rs");
    std::fs::write(&file, TEXT).unwrap();
    crate::qa_workbench_tests::open(&wb, &project, cx);
    crate::qa_workbench_tests::open_in(&wb, &file, cx);
    cx.simulate_resize(size(px(1280.), px(800.)));
    cx.run_until_parked();
    (wb, cx, file)
}

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn zoom_to(wb: &Entity<Workbench>, cx: &mut VisualTestContext, steps: usize) {
    for _ in 0..steps {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Zoom In", window, cx));
        cx.run_until_parked();
    }
}

fn find<'a>(node: NodeRef<'a>, id: NodeId) -> Option<NodeRef<'a>> {
    if node.locate().0 == id {
        return Some(node);
    }
    node.children().find_map(|child| find(child, id))
}

fn center(rect: Rect) -> Point {
    Point::new((rect.x0 + rect.x1) / 2., (rect.y0 + rect.y1) / 2.)
}

fn inside(inner: Rect, outer: Rect) -> bool {
    let eps = 0.01;
    inner.x0 >= outer.x0 - eps && inner.y0 >= outer.y0 - eps && inner.x1 <= outer.x1 + eps && inner.y1 <= outer.y1 + eps
}

/// The physical rectangle of logical `bounds` in the window of `cx`.
fn physical(bounds: gpui::Bounds<Pixels>, cx: &mut VisualTestContext) -> Rect {
    let scale = cx.update(|window, _| window.scale_factor());
    crate::painted::physical_scaled(bounds, scale).expect("a painted rectangle")
}

fn near(a: Rect, b: Rect) -> bool {
    [(a.x0, b.x0), (a.y0, b.y0), (a.x1, b.x1), (a.y1, b.y1)].iter().all(|(x, y)| (x - y).abs() < 0.01)
}

/// Explorer rows, editor tabs and status items: each node's bounds are its painted frame, inside its landmark, and a
/// hit test at its centre finds it; at 200% the frames are twice as large.
#[gpui::test]
fn rows_tabs_and_status_items_have_their_painted_frames(cx: &mut TestAppContext) {
    let (wb, cx, _) = rig(cx, "a11y-frames-rows");
    let mut widths = Vec::new();
    for steps in [0, 10] {
        zoom_to(&wb, cx, steps);
        let snap = snapshot(&wb, cx);
        let explorer = snap.explorers[0].clone();
        let framed: Vec<&RowSnap> = explorer.rows.iter().filter(|r| r.frame.is_some()).collect();
        assert!(framed.len() >= 3, "the painted rows have frames: {:?}", explorer.rows);
        let tree = Tree::new(snap.to_tree(), true);
        let state = tree.state();
        let explorer_bounds = find(state.root(), EXPLORER).and_then(|n| n.data().bounds()).expect("explorer bounds");
        for row in &framed {
            let frame = row.frame.unwrap();
            assert!(inside(frame, explorer_bounds), "row {} {frame:?} inside {explorer_bounds:?}", row.name);
            let hit = state.root().node_at_point(center(frame), &|_| FilterResult::Include).expect("a row is hit");
            assert_eq!((hit.role(), hit.label().as_deref()), (Role::TreeItem, Some(row.name.as_str())), "hit at the centre of {}", row.name);
        }
        let first = framed[0].frame.unwrap();
        widths.push(first.height());

        let window = cx.update(|window, _| window.window_handle().window_id());
        let tab = find(state.root(), NodeId(TAB_BASE)).expect("the tab");
        let painted = wb.read_with(cx, |wb, _| wb.chrome.a11y_bounds.part(window, Part::EditorTab(0))).expect("tab painted");
        assert!(near(tab.data().bounds().expect("tab bounds"), physical(painted, cx)), "the tab's frame is where it is painted");
        let status = find(state.root(), STATUS).and_then(|n| n.data().bounds()).expect("status bounds");
        let problems = snap.status.iter().position(|s| s.contains("errors")).expect("the problems item");
        let item = find(state.root(), NodeId(STATUS_ITEM + problems as u64)).expect("the problems node");
        let item_bounds = item.data().bounds().expect("problems item bounds");
        let painted = wb.read_with(cx, |wb, _| wb.chrome.a11y_bounds.part(window, Part::Status(StatusPart::Problems))).expect("painted");
        assert!(near(item_bounds, physical(painted, cx)), "the status item's frame is where it is painted");
        assert!(inside(item_bounds, status), "{item_bounds:?} inside {status:?}");
        let hit = state.root().node_at_point(center(item_bounds), &|_| FilterResult::Include).unwrap();
        assert_eq!(hit.locate().0, NodeId(STATUS_ITEM + problems as u64));
    }
    assert!((widths[1] - widths[0] * 2.).abs() < 0.5, "a row is twice as tall at 200%: {widths:?}");
}

/// The caret's and a selection's bounds, asked of AccessKit the way VoiceOver and UI Automation ask, are where the
/// editor draws them (the IME's caret rectangle, which comes from the same paint), with non-ASCII text, at 100% and
/// 200%.
#[gpui::test]
fn caret_and_selection_bounds_are_where_the_text_is_painted(cx: &mut TestAppContext) {
    let (wb, cx, _) = rig(cx, "a11y-frames-caret");
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    // "café": line 2 starts at 13; the caret before 'f' of "café" and the selection of "café".
    let line2 = TEXT.lines().next().unwrap().chars().count() + 1;
    let cafe = line2 + "    let ".chars().count();
    for steps in [0, 10] {
        zoom_to(&wb, cx, steps);
        let display = cx.update(|window, _| f64::from(window.scale_factor() / window.ui_scale()));
        ed.update(cx, |e, cx| e.set_cursor_for_test(cafe, cx));
        cx.run_until_parked();
        let at = |cx: &mut VisualTestContext, ix: usize| cx.platform_bounds_for_range(ix..ix).expect("the editor's caret rectangle");
        let (start, end) = (at(cx, cafe), at(cx, cafe + 4));
        let snap = snapshot(&wb, cx);
        let tree = Tree::new(snap.to_tree(), true);
        let state = tree.state();
        let editor = find(state.root(), EDITOR).expect("the editor");
        let caret = editor.text_selection().expect("the caret").bounding_boxes();
        assert_eq!(caret.len(), 1, "one rectangle for the caret: {caret:?}");
        let caret = caret[0];
        // Platform points × display scale = AccessKit's physical pixels.
        assert!((caret.x0 - f64::from(start.origin.x) * display).abs() < 1., "caret x {caret:?} vs {start:?} × {display}");
        assert!((caret.y0 - f64::from(start.origin.y) * display).abs() < 1., "caret y {caret:?} vs {start:?}");
        assert!((caret.height() - f64::from(start.size.height) * display).abs() < 1., "caret height {caret:?} vs {start:?}");
        assert!(inside(caret, editor.data().bounds().expect("editor bounds")), "the caret is inside the editor");

        // The screen reader selects "café": its bounds run from the caret before 'c' to the one after 'é'.
        let editor_snap = snap.editor.as_ref().unwrap();
        let (a, f) = (position_of(&editor_snap.runs, RUN_BASE, cafe).unwrap(), position_of(&editor_snap.runs, RUN_BASE, cafe + 4).unwrap());
        let req = ActionRequest {
            action: Action::SetTextSelection,
            target_tree: TreeId::ROOT,
            target_node: EDITOR,
            data: Some(ActionData::SetTextSelection(TextSelection { anchor: a, focus: f })),
        };
        wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
        cx.run_until_parked();
        assert_eq!(ed.read_with(cx, |e, _| e.selected_text()).as_deref(), Some("café"));
        let snap = snapshot(&wb, cx);
        let tree = Tree::new(snap.to_tree(), true);
        let state = tree.state();
        let boxes = find(state.root(), EDITOR).unwrap().text_selection().unwrap().bounding_boxes();
        assert_eq!(boxes.len(), 1, "one run: {boxes:?}");
        assert!((boxes[0].x0 - f64::from(start.origin.x) * display).abs() < 1., "selection start {boxes:?} vs {start:?}");
        assert!((boxes[0].x1 - f64::from(end.origin.x) * display).abs() < 1., "selection end {boxes:?} vs {end:?}");
    }
}

/// A selection over a line break, and the caret at the end of the document: rectangles for each line, none empty.
#[gpui::test]
fn ranges_over_line_ends_have_a_rectangle_per_line(cx: &mut TestAppContext) {
    let (wb, cx, _) = rig(cx, "a11y-frames-lines");
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.set_cursor_for_test(TEXT.chars().count(), cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let tree = Tree::new(snap.to_tree(), true);
    let state = tree.state();
    let editor = find(state.root(), EDITOR).unwrap();
    let boxes = editor.document_range().bounding_boxes();
    // A rectangle for each of the three lines with text, each with its line break.
    assert_eq!(boxes.len(), 3, "a rectangle per line: {boxes:?}");
    for pair in boxes.windows(2) {
        assert!(pair[1].y0 >= pair[0].y1 - 0.01, "lines go down: {pair:?}");
    }
    assert!(boxes.iter().all(|b| b.width() > 0. && b.height() > 0.), "{boxes:?}");
    // The caret on the empty last line: below the third line, at the left edge of the text.
    let caret = editor.text_selection().unwrap().bounding_boxes();
    assert_eq!(caret.len(), 1, "{caret:?}");
    assert!(caret[0].y0 >= boxes[2].y1 - 0.01 && (caret[0].x0 - boxes[0].x0).abs() < 0.01, "caret {caret:?} after {boxes:?}");
}

/// An editor in its own window: its text runs have frames in that window's coordinates, and the caret's bounds are
/// where that window draws it.
#[gpui::test]
fn an_editor_window_has_caret_bounds_of_its_own(cx: &mut TestAppContext) {
    let (wb, vcx, _) = rig(cx, "a11y-frames-window");
    wb.update_in(vcx, |wb, window, cx| wb.editor_into_new_window(false, window, cx));
    vcx.run_until_parked();
    let handle = wb.read_with(vcx, |wb, _| wb.chrome.editor_windows[0].window);
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    win.simulate_keystrokes("down end");
    win.run_until_parked();
    let root = handle.update(&mut win, |_, _, cx| cx.entity()).unwrap();
    let snap = win.update(|window, cx| windows_access::editor_window_snapshot(&root, window, cx));
    let editor = snap.editor.as_ref().unwrap();
    assert!(editor.runs.iter().all(|r| r.frame.is_some()), "every line is painted in that window");
    let display = win.update(|window, _| f64::from(window.scale_factor() / window.ui_scale()));
    let caret_at = win.platform_bounds_for_range(editor.selection.1..editor.selection.1).expect("the caret rectangle");
    let tree = Tree::new(snap.to_tree(), true);
    let state = tree.state();
    let node = find(state.root(), EDITOR).unwrap();
    let caret = node.text_selection().unwrap().bounding_boxes()[0];
    assert!((caret.x0 - f64::from(caret_at.origin.x) * display).abs() < 1., "{caret:?} vs {caret_at:?}");
    assert!((caret.y0 - f64::from(caret_at.origin.y) * display).abs() < 1., "{caret:?} vs {caret_at:?}");
    assert!(inside(caret, state.root().data().bounds().unwrap()), "inside that window");
    // The main window does not draw it: no character positions from an earlier frame there.
    let main = snapshot(&wb, vcx);
    assert!(main.editor.iter().flat_map(|e| &e.runs).all(|r| r.frame.is_none()), "no stale positions in the main window");
}
