//! End-to-end Explorer selection and file-operation regressions.

use gpui::{TestAppContext, VisualTestContext};

use crate::qa_workbench_tests::{fresh, open, setup};
use crate::tree::TreeEvent;

#[gpui::test]
fn explorer_bulk_keyboard_range_copies_every_selected_file(cx: &mut TestAppContext) {
    let dir = fresh("explorer-range-copy");
    std::fs::write(dir.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(dir.join("b.txt"), "bravo\n").unwrap();
    std::fs::write(dir.join("c.txt"), "charlie\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    crate::qa_integration_tests::wait_ticking(cx, "three Explorer files", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).len() == 3);
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("home shift-down secondary-c");
    cx.run_until_parked();
    let target = dir.join("destination");
    std::fs::create_dir(&target).unwrap();
    wb.update_in(cx, |wb, window, cx| {
        wb.tree_event_for_test(TreeEvent::Paste { dir: crate::ui::local_path(&target) }, window, cx);
    });
    cx.run_until_parked();
    if cx.has_pending_prompt() {
        cx.simulate_prompt_answer("Copy");
    }
    crate::qa_integration_tests::wait_ticking(cx, "the selected copies", |_| target.join("a.txt").exists() && target.join("b.txt").exists());
    assert_eq!(std::fs::read_to_string(target.join("a.txt")).unwrap(), "alpha\n");
    assert_eq!(std::fs::read_to_string(target.join("b.txt")).unwrap(), "bravo\n");
    assert!(!target.join("c.txt").exists());
}

#[gpui::test]
fn explorer_bulk_select_all_exposes_selected_rows(cx: &mut TestAppContext) {
    let dir = fresh("explorer-select-all");
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join(name), name).unwrap();
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    crate::qa_integration_tests::wait_ticking(cx, "three Explorer rows", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).len() == 3);
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("home secondary-a");
    let selected = selected_rows(&wb, cx);
    assert_eq!(selected, ["a.txt", "b.txt", "c.txt"]);
}

fn selected_rows(wb: &gpui::Entity<crate::workbench::Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
        .to_tree()
        .nodes
        .into_iter()
        .filter(|(_, n)| n.role() == accesskit::Role::TreeItem && n.is_selected() == Some(true))
        .filter_map(|(_, n)| n.label().map(str::to_string))
        .collect()
}

#[gpui::test]
fn explorer_bulk_ranges_shrink_and_selection_survives_refresh(cx: &mut TestAppContext) {
    let dir = fresh("explorer-range-refresh");
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        std::fs::write(dir.join(name), name).unwrap();
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    crate::qa_integration_tests::wait_ticking(cx, "four rows", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).len() == 4);
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("home shift-end shift-up");
    assert_eq!(selected_rows(&wb, cx), ["a.txt", "b.txt", "c.txt"]);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("File: Refresh Explorer", window, cx));
    cx.run_until_parked();
    assert_eq!(selected_rows(&wb, cx), ["a.txt", "b.txt", "c.txt"]);
    cx.simulate_keystrokes("secondary-space");
    assert_eq!(selected_rows(&wb, cx), ["a.txt", "b.txt"]);
    cx.simulate_keystrokes("home secondary-space secondary-c");
    assert!(selected_rows(&wb, cx).is_empty());
    assert!(wb.read_with(cx, |wb, _| wb.file_clipboard_paths_for_test()).is_empty());
}

#[gpui::test]
fn explorer_bulk_cut_keeps_failed_entries_for_retry_and_moves_dirty_tabs(cx: &mut TestAppContext) {
    let dir = fresh("explorer-cut-retry");
    let target = fresh("explorer-cut-retry-target");
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(name), name).unwrap();
    }
    std::fs::write(target.join("b.txt"), "keep this").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("a.txt"), cx);
    cx.simulate_input("unsaved-");
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("home shift-end secondary-x");
    wb.update_in(cx, |wb, window, cx| wb.tree_event_for_test(TreeEvent::Paste { dir: crate::ui::local_path(&target) }, window, cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Move");
    wait_idle(&wb, cx);
    assert!(!dir.join("a.txt").exists());
    assert!(dir.join("b.txt").exists());
    assert_eq!(std::fs::read_to_string(target.join("b.txt")).unwrap(), "keep this");
    assert_eq!(wb.read_with(cx, |wb, _| wb.file_clipboard_paths_for_test()), [crate::ui::local_path(&dir.join("b.txt"))]);
    let editor = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    assert!(editor.read_with(cx, |e, _| e.is_dirty()));
    assert_eq!(editor.read_with(cx, |e, _| e.path.clone()), crate::ui::local_path(&target.join("a.txt")));
    assert!(editor.read_with(cx, |e, _| e.text()).contains("unsaved-"));
    std::fs::remove_file(target.join("b.txt")).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.tree_event_for_test(TreeEvent::Paste { dir: crate::ui::local_path(&target) }, window, cx));
    wait_idle(&wb, cx);
    assert!(!dir.join("b.txt").exists());
    assert_eq!(std::fs::read_to_string(target.join("b.txt")).unwrap(), "b.txt");
    assert!(wb.read_with(cx, |wb, _| wb.file_clipboard_paths_for_test()).is_empty());
}

#[gpui::test]
fn explorer_bulk_delete_has_one_confirmation_and_preserves_dirty_buffers(cx: &mut TestAppContext) {
    let dir = fresh("explorer-delete-dirty");
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(name), name).unwrap();
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("a.txt"), cx);
    cx.simulate_input("unsaved-");
    let paths = vec![crate::ui::local_path(&dir.join("a.txt")), crate::ui::local_path(&dir.join("b.txt"))];
    wb.update_in(cx, |wb, window, cx| wb.tree_event_for_test(TreeEvent::DeleteMany(paths.clone()), window, cx));
    cx.run_until_parked();
    crate::question::cancel_keeps_focus(&wb, cx, "Cancel");
    wait_idle(&wb, cx);
    assert!(dir.join("a.txt").exists() && dir.join("b.txt").exists());
    wb.update_in(cx, |wb, window, cx| wb.tree_event_for_test(TreeEvent::DeleteMany(paths), window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Delete Permanently");
    wait_idle(&wb, cx);
    assert!(!dir.join("a.txt").exists() && !dir.join("b.txt").exists());
    assert!(!cx.has_pending_prompt());
    let editor = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    assert!(editor.read_with(cx, |e, _| e.is_dirty()));
    assert!(editor.read_with(cx, |e, _| e.text()).contains("unsaved-"));
}

#[gpui::test]
fn explorer_bulk_confirmation_is_invalidated_by_workspace_changes(cx: &mut TestAppContext) {
    let dir = fresh("explorer-stale-confirmation");
    let target = fresh("explorer-stale-target");
    let other = fresh("explorer-stale-other-root");
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(name), name).unwrap();
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let paths = ["a.txt", "b.txt"].map(|n| crate::ui::local_path(&dir.join(n))).to_vec();
    wb.update_in(cx, |wb, window, cx| {
        wb.tree_event_for_test(TreeEvent::CopyMany(paths), window, cx);
        wb.tree_event_for_test(TreeEvent::Paste { dir: crate::ui::local_path(&target) }, window, cx);
    });
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    open(&wb, &other, cx);
    cx.simulate_prompt_answer("Copy");
    cx.run_until_parked();
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    assert!(!wb.read_with(cx, |wb, _| wb.file_operation_active_for_test()));
}

#[gpui::test]
fn explorer_bulk_cancel_before_confirmation_prevents_every_side_effect(cx: &mut TestAppContext) {
    let dir = fresh("explorer-cancel-before-start");
    let target = fresh("explorer-cancel-before-start-target");
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(name), name).unwrap();
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let paths = ["a.txt", "b.txt"].map(|n| crate::ui::local_path(&dir.join(n))).to_vec();
    wb.update_in(cx, |wb, window, cx| {
        wb.tree_event_for_test(TreeEvent::CutMany(paths.clone()), window, cx);
        wb.tree_event_for_test(TreeEvent::Paste { dir: crate::ui::local_path(&target) }, window, cx);
        wb.run_command_for_test("File: Cancel File Operation", window, cx);
    });
    cx.run_until_parked();
    cx.simulate_prompt_answer("Move");
    wait_idle(&wb, cx);
    assert!(dir.join("a.txt").exists() && dir.join("b.txt").exists());
    assert_eq!(std::fs::read_dir(target).unwrap().count(), 0);
    assert_eq!(wb.read_with(cx, |wb, _| wb.file_clipboard_paths_for_test()), paths);
}

fn wait_idle(wb: &gpui::Entity<crate::workbench::Workbench>, cx: &mut VisualTestContext) {
    crate::qa_integration_tests::wait_ticking(cx, "the file operation to finish", |cx| !wb.read_with(cx, |wb, _| wb.file_operation_active_for_test()));
}
