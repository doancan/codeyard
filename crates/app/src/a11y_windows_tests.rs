//! The accessibility trees of an editor's and a terminal's own window.

use super::*;
use crate::qa_workbench_tests::{fresh, open, open_in, setup};
use gpui::VisualTestContext;

/// AC8: the editor's own window exposes its title and the editor as a text field with the caret of THAT window's view;
/// AccessKit's consumer accepts the tree, and a selection set by the screen reader lands in that view.
#[gpui::test]
fn an_editor_window_exposes_its_editor_and_caret(cx: &mut gpui::TestAppContext) {
    let dir = fresh("a11y-edwin");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();
    let (wb, vcx) = setup(cx);
    open(&wb, &dir, vcx);
    open_in(&wb, &file, vcx);
    let ed = wb.read_with(vcx, |wb, _| wb.active_editor().cloned()).expect("an editor");
    wb.update_in(vcx, |wb, window, cx| wb.editor_into_new_window(true, window, cx));
    vcx.run_until_parked();
    let (handle, slot) = wb.read_with(vcx, |wb, _| (wb.chrome.editor_windows[0].window, wb.chrome.editor_windows[0].slot));
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    win.simulate_keystrokes("down");
    win.run_until_parked();
    let root = handle.update(&mut win, |_, _, cx| cx.entity()).unwrap();
    let snap = win.update(|window, cx| editor_window_snapshot(&root, window, cx));
    assert_eq!(snap.title, "notes.txt");
    let editor = snap.editor.as_ref().expect("the editor is in the tree");
    assert_eq!(editor.selection, (6, 6), "the caret of this window's view (line 2), not the tab's");
    assert_eq!(snap.focus, FocusSnap::Editor);
    let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
    assert_eq!(tree.state().root().role(), Role::Window);
    assert_eq!(tree.state().focus().unwrap().role(), Role::MultilineTextInput);
    // The screen reader selects "gamma": the window's view selects it, the tab's caret stays.
    let (a, f) = (position_of(&editor.runs, RUN_BASE, 11).unwrap(), position_of(&editor.runs, RUN_BASE, 16).unwrap());
    let req = ActionRequest {
        action: Action::SetTextSelection,
        target_tree: TreeId::ROOT,
        target_node: EDITOR,
        data: Some(ActionData::SetTextSelection(TextSelection { anchor: a, focus: f })),
    };
    win.update(|window, cx| (EDITOR_WINDOW.action)(root.clone().into(), &req, &snap, window, cx));
    win.run_until_parked();
    assert_eq!(ed.update(&mut win, |e, _| e.in_slot(slot, |e| e.selected_text())).as_deref(), Some("gamma"));
    assert_eq!(ed.update(&mut win, |e, _| e.view_for_slot(0).line), 0, "the tab's view did not move");
}

/// A terminal's own window exposes the terminal with the keyboard in it.
#[gpui::test]
fn a_terminal_window_exposes_its_terminal(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = setup(cx);
    wb.update_in(vcx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    vcx.run_until_parked();
    wb.update_in(vcx, |wb, window, cx| wb.detach_terminal(window, cx));
    vcx.run_until_parked();
    let handle = wb.read_with(vcx, |wb, _| wb.chrome.detached[0].window);
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    let root = handle.update(&mut win, |_, _, cx| cx.entity()).unwrap();
    let snap = win.update(|window, cx| terminal_window_snapshot(&root, window, cx));
    assert!(snap.terminal.is_some(), "the terminal is in the tree");
    assert_eq!(snap.focus, FocusSnap::Terminal);
    let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
    assert_eq!(tree.state().root().role(), Role::Window);
    wb.update(vcx, |wb, cx| wb.close_all_terminals(cx));
}
