use super::*;
use gpui::{TestAppContext, VisualTestContext};

fn opened_tabs<'a>(cx: &'a mut TestAppContext, name: &str) -> (Entity<Workbench>, &'a mut VisualTestContext, Vec<Entity<EditorView>>) {
    let root = crate::qa_workbench_tests::fresh(name);
    for file in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(root.join(file), file).unwrap();
    }
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update(cx, |wb, cx| wb.replace_root(crate::ui::local_path(&root), cx));
    for file in ["a.txt", "b.txt", "c.txt"] {
        wb.update_in(cx, |wb, window, cx| wb.open_file(crate::ui::local_path(&root.join(file)), window, cx));
        cx.run_until_parked();
    }
    let editors = wb.read_with(cx, |wb, _| wb.tabs.editors.clone());
    assert_eq!(editors.len(), 3);
    (wb, cx, editors)
}

#[gpui::test]
fn a_tab_close_menu_cannot_retarget_after_tabs_reorder_or_close(cx: &mut TestAppContext) {
    check_tab_menu(cx, false);
}

#[gpui::test]
fn a_tab_pin_menu_cannot_retarget_after_tabs_reorder_or_close(cx: &mut TestAppContext) {
    check_tab_menu(cx, true);
}

#[gpui::test]
fn an_old_tab_menu_cannot_pin_after_its_host_or_workspace_changes(cx: &mut TestAppContext) {
    for change_host in [false, true] {
        let (wb, cx, editors) = opened_tabs(cx, &format!("tab-menu-context-{change_host}"));
        let other = crate::qa_workbench_tests::fresh(&format!("tab-menu-other-root-{change_host}"));
        wb.update_in(cx, |wb, window, cx| {
            wb.open_tab_menu(1, gpui::point(px(0.), px(0.)), window, cx);
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.replace_root(crate::ui::local_path(&other), cx);
            }
            wb.select_menu_for_test("Pin", cx);
        });
        cx.run_until_parked();
        assert!(!editors[1].read_with(cx, |editor, _| editor.pinned));
        assert!(wb.read_with(cx, |wb, _| wb.menu.is_none()));
    }
}

#[gpui::test]
fn a_tab_menu_closes_the_original_group_view_of_a_shared_editor(cx: &mut TestAppContext) {
    let (wb, cx, editors) = opened_tabs(cx, "tab-menu-shared-editor");
    let (source, other) = wb.update_in(cx, |wb, window, cx| {
        wb.activate(1, window, cx);
        let source = wb.tabs.current_key();
        wb.open_tab_menu(1, gpui::point(px(0.), px(0.)), window, cx);
        wb.split_editor(&SplitEditor, window, cx);
        let other = wb.tabs.current_key();
        assert_ne!(source, other);
        wb.select_menu_for_test("Close", cx);
        (source, other)
    });
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| !wb.tabs.group(source.id).unwrap().editors.contains(&editors[1])));
    assert!(wb.read_with(cx, |wb, _| wb.tabs.group(other.id).unwrap().editors.contains(&editors[1])), "the other group's shared view must stay open");
}

#[gpui::test]
fn an_old_tab_menu_cannot_close_a_recreated_group_with_the_same_editor(cx: &mut TestAppContext) {
    let (wb, cx, editors) = opened_tabs(cx, "tab-menu-recreated-group");
    let (old, replacement) = wb.update_in(cx, |wb, window, cx| {
        wb.activate(1, window, cx);
        wb.split_editor(&SplitEditor, window, cx);
        let old = wb.tabs.current_key();
        wb.open_tab_menu(0, gpui::point(px(0.), px(0.)), window, cx);
        wb.remove_editor(0, window, cx);
        assert!(!wb.tabs.contains_key(old));
        wb.split_editor(&SplitEditor, window, cx);
        let replacement = wb.tabs.current_key();
        assert_ne!(old, replacement);
        wb.select_menu_for_test("Close", cx);
        (old, replacement)
    });
    cx.run_until_parked();
    assert!(!wb.read_with(cx, |wb, _| wb.tabs.contains_key(old)));
    assert!(wb.read_with(cx, |wb, _| wb.tabs.contains_key(replacement)), "an old group lifetime cannot target its replacement");
    assert!(wb.read_with(cx, |wb, _| wb.tabs.group(replacement.id).unwrap().editors.contains(&editors[1])));
}

#[gpui::test]
fn tab_bulk_close_resolves_the_original_view_after_reordering(cx: &mut TestAppContext) {
    for others in [false, true] {
        let (wb, cx, editors) = opened_tabs(cx, &format!("tab-menu-bulk-{others}"));
        wb.update_in(cx, |wb, window, cx| {
            wb.open_tab_menu(1, gpui::point(px(0.), px(0.)), window, cx);
            wb.drop_tab_for_test(true, "b.txt", true, Some("a.txt"), window, cx);
            wb.select_menu_for_test(if others { "Close Others" } else { "Close to the Right" }, cx);
        });
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.editors.clone()), vec![editors[1].clone()]);
    }
}

fn check_tab_menu(cx: &mut TestAppContext, pin: bool) {
    for change in 0..4 {
        let (wb, cx, editors) = opened_tabs(cx, &format!("tab-menu-{pin}-{change}"));
        wb.update_in(cx, |wb, window, cx| {
            wb.open_tab_menu(1, gpui::point(px(0.), px(0.)), window, cx);
            match change {
                0 => wb.drop_tab_for_test(true, "b.txt", true, Some("a.txt"), window, cx),
                1 => wb.remove_editor(0, window, cx),
                2 => wb.remove_editor(1, window, cx),
                _ => {}
            }
            wb.select_menu_for_test(if pin { "Pin" } else { "Close" }, cx);
        });
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| wb.tabs.editors.contains(&editors[2])), "the old menu must not close a different tab");
        if change != 1 {
            assert!(wb.read_with(cx, |wb, _| wb.tabs.editors.contains(&editors[0])));
        }
        assert!(!editors[0].read_with(cx, |editor, _| editor.pinned));
        assert!(!editors[2].read_with(cx, |editor, _| editor.pinned), "the old menu must not pin a different tab");
        if pin {
            assert_eq!(editors[1].read_with(cx, |editor, _| editor.pinned), change != 2);
            assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.editors.contains(&editors[1])), change != 2);
        } else {
            assert!(!wb.read_with(cx, |wb, _| wb.tabs.editors.contains(&editors[1])), "Close must still target the original view when it exists");
        }
        assert!(wb.read_with(cx, |wb, _| wb.menu.is_none()));
    }
}
