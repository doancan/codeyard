//! User-visible group behavior. Layout-model checks alone do not establish editing support.

use gpui::TestAppContext;

use crate::qa_workbench_tests::{fresh, open, setup};

#[gpui::test]
fn editor_grid_repeated_split_creates_three_independent_views_of_one_document(cx: &mut TestAppContext) {
    let dir = fresh("editor-grid-three-views");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "alpha\nbravo\ncharlie\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &file, cx);
    cx.simulate_keystrokes("cmd-\\");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-\\");
    cx.run_until_parked();
    let editor = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let views = editor.read_with(cx, |e, _| e.painted_lines());
    assert_eq!(views.len(), 3, "each split adds a group with its own view");
    assert!(views.iter().all(|lines| *lines > 0));
}
