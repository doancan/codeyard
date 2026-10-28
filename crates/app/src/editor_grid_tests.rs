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

use super::*;
use crate::qa_workbench_tests::{open_in, restart_with, setup_persistent};
use gpui::{VisualTestContext, point, size};

fn run(wb: &Entity<Workbench>, command: &str, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(command, window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn editor_grid_three_views_share_edits_and_keep_independent_cursors(cx: &mut TestAppContext) {
    let dir = fresh("editor-grid-cursors");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor().cloned()).unwrap();
    cx.simulate_keystrokes("down cmd-\\ down cmd-\\ down");
    cx.run_until_parked();
    cx.simulate_input("C");
    run(&wb, "View: Focus Left Editor Group", cx);
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()), (3, 1));
    cx.simulate_input("B");
    run(&wb, "View: Focus Left Editor Group", cx);
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()), (2, 1));
    cx.simulate_input("A");
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "one\nAtwo\nBthree\nC");
    assert_eq!(wb.read_with(cx, |wb, _| wb.all_editors().len()), 1);
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "one\ntwo\nBthree\nC");
}

#[gpui::test]
fn editor_grid_mixed_axes_paint_nonoverlapping_panes_at_narrow_and_wide_sizes(cx: &mut TestAppContext) {
    let dir = fresh("editor-grid-geometry");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "hello\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    run(&wb, "View: Split Editor", cx);
    run(&wb, "View: Split Editor Down", cx);
    for width in [800., 1280.] {
        cx.simulate_resize(size(px(width), px(800.)));
        cx.run_until_parked();
        let ids = wb.read_with(cx, |wb, _| wb.tabs.layout.groups());
        assert_eq!(ids.len(), 3);
        let panes: Vec<_> = ["editor-group-0", "editor-group-1", "editor-group-2"].map(|name| cx.debug_bounds(name).unwrap()).into_iter().collect();
        assert!(panes.iter().all(|b| b.size.width > px(0.) && b.size.height > px(0.)));
        assert_eq!(panes[1].left(), panes[2].left());
        assert!(panes[0].right() <= panes[1].left());
        assert!(panes[1].bottom() <= panes[2].top());
        run(&wb, "View: Focus Upper Editor Group", cx);
        assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.layout.focused()), 1);
        run(&wb, "View: Focus Lower Editor Group", cx);
        assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.layout.focused()), 2);
    }
}

#[gpui::test]
fn editor_grid_drag_targets_stable_groups_and_stale_events_cannot_hit_reused_slots(cx: &mut TestAppContext) {
    let dir = fresh("editor-grid-stale-drag");
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(name), name).unwrap();
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("a.txt"), cx);
    open_in(&wb, &dir.join("b.txt"), cx);
    run(&wb, "View: Split Editor", cx);
    run(&wb, "View: Split Editor Down", cx);
    let (old, target, editor) = wb.read_with(cx, |wb, _| (wb.tabs.current_key(), wb.tabs.key(0).unwrap(), wb.active_editor().cloned().unwrap()));
    wb.update_in(cx, |wb, window, cx| wb.drop_tab(editor.clone(), old, target, None, window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.layout.groups().len()), 2);
    run(&wb, "View: Split Editor Down", cx);
    let replacement = wb.read_with(cx, |wb, _| wb.tabs.current_key());
    assert_eq!(replacement.id, old.id);
    assert_ne!(replacement.epoch, old.epoch);
    let before = wb.read_with(cx, |wb, _| wb.editor_grid_snapshot());
    wb.update_in(cx, |wb, window, cx| wb.drop_tab(editor, old, target, None, window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.editor_grid_snapshot()), before);
}

#[gpui::test]
fn editor_grid_nested_divider_uses_its_own_bounds_and_persists_ratio(cx: &mut TestAppContext) {
    let dir = fresh("editor-grid-resize");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "hello\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    run(&wb, "View: Split Editor", cx);
    run(&wb, "View: Split Editor Down", cx);
    let sash = cx.debug_bounds("group-sash-1").unwrap();
    let first = cx.debug_bounds("editor-group-1").unwrap();
    let second = cx.debug_bounds("editor-group-2").unwrap();
    let target = point(sash.center().x, first.top() + (second.bottom() - first.top()) * 0.7);
    cx.simulate_mouse_down(sash.center(), MouseButton::Left, gpui::Modifiers::none());
    cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::none());
    cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::none());
    cx.run_until_parked();
    let ratio = wb.read_with(cx, |wb, _| wb.tabs.layout.rects().into_iter().find(|(id, _)| *id == 1).unwrap().1.height);
    assert!((ratio - 0.7).abs() < 0.02, "nested split ratio {ratio}");
    assert!(wb.read_with(cx, |wb, _| wb.tabs.resizing.is_none()));
}

#[gpui::test]
fn editor_grid_restart_restores_topology_focus_shared_documents_and_per_group_views(cx: &mut TestAppContext) {
    let home = fresh("editor-grid-restore-home");
    let dir = fresh("editor-grid-restore-project");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    open_in(&wb, &file, cx);
    cx.simulate_keystrokes("down");
    run(&wb, "View: Split Editor", cx);
    cx.simulate_keystrokes("down");
    run(&wb, "View: Split Editor Down", cx);
    cx.simulate_keystrokes("down");
    let grid = wb.read_with(cx, |wb, _| wb.tabs.layout.clone());
    let wb = restart_with(wb, cx, &home, crate::settings::Startup::Continue);
    assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.layout.clone()), grid);
    assert_eq!(wb.read_with(cx, |wb, _| wb.all_editors().len()), 1);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor().cloned()).unwrap();
    for (id, expected) in [(0, 1), (1, 2), (2, 3)] {
        let view = ed.update(cx, |e, _| e.view_for_slot(id));
        assert_eq!(view.line, expected, "slot {id}");
    }
}

#[gpui::test]
fn editor_grid_empty_groups_restore_and_special_tabs_stay_with_their_group(cx: &mut TestAppContext) {
    let home = fresh("editor-grid-empty-home");
    let dir = fresh("editor-grid-empty-project");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    run(&wb, "View: Split Editor", cx);
    run(&wb, "View: Split Editor Down", cx);
    let grid = wb.read_with(cx, |wb, _| wb.tabs.layout.clone());
    let wb = restart_with(wb, cx, &home, crate::settings::Startup::Continue);
    assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.layout.clone()), grid);
    run(&wb, "View: Close Empty Editor Group", cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.layout.groups().len()), 2);
    run(&wb, "Preferences: Open Settings", cx);
    let owner = wb.read_with(cx, |wb, _| wb.tabs.current_key());
    let page = wb.read_with(cx, |wb, _| wb.tabs.page.is_some());
    assert!(page, "settings UI opens a group-local page");
    wb.update_in(cx, |wb, window, cx| {
        let other = wb.tabs.layout.groups().into_iter().find(|id| *id != owner.id).unwrap();
        wb.focus_group_key(wb.tabs.key(other).unwrap(), window, cx);
    });
    assert!(wb.read_with(cx, |wb, _| wb.tabs.page.is_none()));
    assert!(wb.read_with(cx, |wb, _| wb.tabs.group(owner.id).unwrap().page.is_some()));
}

#[gpui::test]
fn editor_grid_many_groups_remain_usable_and_focus_scrolls_the_destination_into_view(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    cx.simulate_resize(size(px(480.), px(480.)));
    for _ in 1..crate::editor_layout::MAX_GROUPS {
        run(&wb, "View: Split Editor", cx);
    }
    let before = wb.read_with(cx, |wb, _| wb.tabs.layout.clone());
    run(&wb, "View: Split Editor", cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.layout.clone()), before);
    let last = cx.debug_bounds("editor-group-31").unwrap();
    let viewport = wb.read_with(cx, |wb, _| wb.tabs.grid_scroll.bounds());
    assert!(last.size.width >= px(159.) && last.size.height >= px(95.), "{last:?}");
    assert!(last.left() >= viewport.left() && last.right() <= viewport.right() + px(1.), "destination {last:?}, viewport {viewport:?}");
    wb.update_in(cx, |wb, window, cx| wb.focus_group_key(wb.tabs.key(0).unwrap(), window, cx));
    cx.run_until_parked();
    let first = cx.debug_bounds("editor-group-0").unwrap();
    assert!(
        first.left() >= viewport.left() && first.right() <= viewport.right() + px(1.),
        "first {first:?}, viewport {viewport:?}, scroll {:?}, pending {:?}",
        wb.read_with(cx, |wb, _| wb.tabs.grid_scroll.offset()),
        wb.read_with(cx, |wb, _| wb.tabs.reveal_group.get())
    );
}

#[gpui::test]
fn editor_grid_late_file_load_cannot_populate_a_reused_group(cx: &mut TestAppContext) {
    let dir = fresh("editor-grid-late-load");
    let file = dir.join("late.txt");
    std::fs::write(&file, "old group only").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.split_editor(&SplitEditor, window, cx);
        let old = wb.tabs.current_key();
        wb.open_file(crate::ui::local_path(&file), window, cx);
        assert!(wb.tabs.opening.is_some());
        wb.close_empty_editor_group(&CloseEmptyEditorGroup, window, cx);
        wb.split_editor(&SplitEditor, window, cx);
        assert_eq!(wb.tabs.current_key().id, old.id);
        assert_ne!(wb.tabs.current_key().epoch, old.epoch);
    });
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.all_editors().is_empty()));
    assert!(wb.read_with(cx, |wb, _| wb.tabs.open_error.is_none()));
}

#[gpui::test]
fn editor_grid_background_file_completion_preserves_user_focus(cx: &mut TestAppContext) {
    let dir = fresh("editor-grid-background-load");
    let file = dir.join("background.txt");
    std::fs::write(&file, "background document").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let destination = wb.update_in(cx, |wb, window, cx| {
        let source = wb.tabs.current_key();
        wb.split_editor(&SplitEditor, window, cx);
        let destination = wb.tabs.current_key();
        wb.open_file(crate::ui::local_path(&file), window, cx);
        wb.focus_group_key(source, window, cx);
        destination
    });
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.layout.focused()), 0);
    assert!(wb.read_with(cx, |wb, _| wb.tabs.editors.is_empty()));
    assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.group(destination.id).unwrap().editors.len()), 1);
}

#[gpui::test]
fn editor_grid_concurrent_opens_share_one_buffer_and_undo_history(cx: &mut TestAppContext) {
    let dir = fresh("editor-grid-concurrent-opens");
    let file = dir.join("shared.txt");
    std::fs::write(&file, "shared content").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.open_file(crate::ui::local_path(&file), window, cx);
        wb.split_editor(&SplitEditor, window, cx);
        wb.open_file(crate::ui::local_path(&file), window, cx);
    });
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.all_editors().len()), 1);
    wb.read_with(cx, |wb, _| assert_eq!(wb.tabs.editors[0], wb.tabs.groups[&0].editors[0]));
}

#[gpui::test]
fn editor_grid_deep_highlighted_views_have_bounded_layout_work(cx: &mut TestAppContext) {
    let dir = fresh("editor-grid-deep-highlighted");
    let file = dir.join("big.rs");
    std::fs::copy(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/big.rs"), &file).unwrap();
    let (wb, cx) = setup(cx);
    cx.update(|_, cx| cx.global_mut::<Settings>().lsp_enabled = false);
    open(&wb, &file, cx);
    cx.simulate_resize(size(px(480.), px(600.)));
    let started = std::time::Instant::now();
    for _ in 1..crate::editor_layout::MAX_GROUPS {
        run(&wb, "View: Split Editor", cx);
    }
    let editor = wb.read_with(cx, |wb, _| wb.active_editor().cloned()).unwrap();
    let hidden_before = editor::slot_paint_count(&editor, 0);
    let visible_before = editor::slot_paint_count(&editor, 31);
    wb.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(editor::slot_paint_count(&editor, 0), hidden_before, "a fully clipped view must not be painted");
    assert!(editor::slot_paint_count(&editor, 31) > visible_before, "the visible view must repaint");
    for _ in 1..crate::editor_layout::MAX_GROUPS {
        run(&wb, "View: Focus Left Editor Group", cx);
    }
    let elapsed = started.elapsed();
    eprintln!("32 highlighted views and 31 focus changes: {elapsed:?}");
    assert!(elapsed < std::time::Duration::from_secs(15 * crate::test_sshd::timeout_scale()));
    assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.layout.focused()), 0);
    assert_eq!(wb.read_with(cx, |wb, _| wb.all_editors().len()), 1);
}
