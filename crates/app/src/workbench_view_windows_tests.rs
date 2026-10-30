//! Side views in windows of their own: the same view moved out and back, commands and menus there, restarts and the
//! accessibility trees.

use super::*;
use crate::qa_workbench_tests::{fresh, open, setup, setup_persistent};
use gpui::VisualTestContext;

fn window_count(cx: &mut VisualTestContext) -> usize {
    cx.update(|_, cx| cx.windows().len())
}

fn view_window(wb: &Entity<Workbench>, panel: Panel, cx: &mut VisualTestContext) -> gpui::WindowHandle<ViewWindow> {
    wb.read_with(cx, |wb, _| wb.chrome.view_windows.iter().find(|w| w.panel == panel).map(|w| w.window)).expect("the view has a window")
}

/// AC1: the open view moves into a window of its own (the same view: the side bar lets go of it) and closing that
/// window puts it back in the side bar it came from.
#[gpui::test]
fn a_side_view_moves_into_its_own_window_and_back_when_that_closes(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = setup(cx);
    let windows = window_count(vcx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.show_scm(&ShowScm, window, cx);
        wb.view_into_new_window(window, cx);
    });
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert!(wb.in_view_window(Panel::Scm));
        assert_eq!(wb.chrome.panel, None, "the side bar let go of it");
        assert!(wb.view_shown(Panel::Scm), "it is still on screen (its counts refresh)");
    });
    assert_eq!(window_count(vcx), windows + 1);
    let handle = view_window(&wb, Panel::Scm, vcx);
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    assert_eq!(handle.update(&mut win, |root, _, _| root.panel).unwrap(), Panel::Scm, "the window draws Source Control");
    assert!(win.simulate_close());
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert!(!wb.in_view_window(Panel::Scm));
        assert_eq!(wb.chrome.panel, Some(Panel::Scm), "back in the primary side bar");
    });
}

/// A view from the secondary side bar goes back there; "View: Move View Back to Side Bar" in its window returns it.
#[gpui::test]
fn a_view_goes_back_to_the_side_bar_it_came_from(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = setup(cx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.show_explorer(&ShowExplorer, window, cx);
        wb.dock_view(Panel::Testing, true, cx);
        wb.chrome.panel = None;
        wb.view_into_new_window(window, cx);
    });
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| assert!(wb.in_view_window(Panel::Testing) && wb.chrome.secondary.is_none() && wb.chrome.in_secondary.is_empty()));
    let handle = view_window(&wb, Panel::Testing, vcx);
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    win.update(|window, cx| window.dispatch_action(Box::new(MoveViewBackToSideBar), cx));
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert!(!wb.in_view_window(Panel::Testing));
        assert_eq!((wb.chrome.secondary, wb.chrome.in_secondary.clone()), (Some(Panel::Testing), vec![Panel::Testing]), "back in the secondary side bar");
    });
}

/// Asking for a view that has a window (its command, its icon) brings that window forward instead of opening the view
/// in a side bar; the primary side bar keeps its view.
#[gpui::test]
fn asking_for_a_windowed_view_brings_its_window_forward(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = setup(cx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.show_scm(&ShowScm, window, cx);
        wb.view_into_new_window(window, cx);
        wb.show_explorer(&ShowExplorer, window, cx);
        wb.leave_editor_window();
    });
    vcx.run_until_parked();
    wb.update_in(vcx, |wb, window, cx| wb.show_scm(&ShowScm, window, cx));
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert_eq!(wb.chrome.panel, Some(Panel::Explorer), "the side bar kept its view");
        assert_eq!(wb.chrome.view_host, Some(Panel::Scm), "the view's window is the one in use");
    });
    assert!(vcx.debug_bounds("act-scm").is_some());
}

/// While its window is in use, a menu or a picker asked for by the view opens in that window; the palette works there.
#[gpui::test]
fn menus_and_the_palette_open_in_the_views_window(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = setup(cx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.show_explorer(&ShowExplorer, window, cx);
        wb.view_into_new_window(window, cx);
    });
    vcx.run_until_parked();
    let handle = view_window(&wb, Panel::Explorer, vcx);
    // A menu asked for through the main window (as the tree's events arrive) opens in the view's window.
    wb.update_in(vcx, |wb, window, cx| {
        wb.use_view_window(Panel::Explorer, cx);
        wb.show_action_menu(vec![("Copy", Box::new(crate::editor::Copy), false)], gpui::point(px(20.), px(20.)), window, cx);
    });
    vcx.run_until_parked();
    let menu = wb.read_with(vcx, |wb, _| wb.menu.as_ref().map(|m| m.menu.clone())).expect("the menu is open");
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    assert!(win.update(|window, cx| menu.focus_handle(cx).is_focused(window)), "in the view's window");
    win.simulate_keystrokes("escape");
    win.run_until_parked();
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    win.simulate_keystrokes("cmd-shift-p");
    win.run_until_parked();
    let picker = wb.read_with(vcx, |wb, _| wb.overlay.as_ref().map(|o| o.picker.clone())).expect("the palette is open");
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    assert!(win.update(|window, cx| picker.focus_handle(cx).is_focused(window)), "with the keyboard in the view's window");
}

/// The windows close with the folder, their views going back; a restart of the folder brings them back on a display.
#[gpui::test]
fn view_windows_follow_the_folder_and_come_back_after_a_restart(cx: &mut gpui::TestAppContext) {
    let home = fresh("viewwin-restart-home");
    let dir = fresh("viewwin-restart-project");
    let (wb, vcx) = setup_persistent(cx, &home);
    open(&wb, &dir, vcx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.show_explorer(&ShowExplorer, window, cx);
        wb.view_into_new_window(window, cx);
    });
    vcx.run_until_parked();
    let wb = crate::qa_workbench_tests::restart_with(wb, vcx, &home, crate::settings::Startup::Continue);
    let start = std::time::Instant::now();
    while !wb.read_with(vcx, |wb, _| wb.in_view_window(Panel::Explorer)) && start.elapsed() < std::time::Duration::from_secs(10) {
        vcx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(wb.read_with(vcx, |wb, _| wb.in_view_window(Panel::Explorer)), "the Explorer's window is back");
    let handle = view_window(&wb, Panel::Explorer, vcx);
    let display = vcx.update(|_, cx| cx.primary_display().unwrap().bounds());
    let at = vcx.update(|_, cx| handle.update(cx, |_, window, _| window.bounds()).unwrap());
    assert!(display.contains(&at.origin) && at.size.width <= display.size.width, "{at:?} on {display:?}");
    let windows = window_count(vcx);
    wb.update(vcx, |wb, cx| wb.close_folder(cx));
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| assert!(!wb.in_view_window(Panel::Explorer)));
    assert_eq!(window_count(vcx), windows - 1, "the window closed with the folder");
}

/// AC8: the view's part of the tree is in its own window's tree (named after the view) and not in the main window's.
#[gpui::test]
fn the_views_window_has_its_part_of_the_accessibility_tree(cx: &mut gpui::TestAppContext) {
    let dir = fresh("viewwin-a11y");
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    let (wb, vcx) = setup(cx);
    open(&wb, &dir, vcx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.show_explorer(&ShowExplorer, window, cx);
        wb.view_into_new_window(window, cx);
    });
    vcx.run_until_parked();
    let handle = view_window(&wb, Panel::Explorer, vcx);
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    let root = handle.update(&mut win, |_, _, cx| cx.entity()).unwrap();
    let snap = win.update(|window, cx| (a11y::windows_access::VIEW_WINDOW.snapshot)(root.into(), window, cx)).expect("a tree");
    assert_eq!(snap.title, crate::i18n::t("Explorer"));
    assert_eq!(snap.explorers.len(), 1, "the Explorer is in its window's tree");
    accesskit_consumer::Tree::new(snap.to_tree(), true);
    let main = vcx.update(|window, cx| (a11y::windows_access::WORKBENCH.snapshot)(wb.clone().into(), window, cx)).unwrap();
    assert!(main.explorers.is_empty(), "and not in the main window's");
}

/// The Search results are in the accessibility tree wherever the Search view is: the primary side bar, the secondary
/// one, or its own window (and then only in that window's tree).
#[gpui::test]
fn search_results_are_in_the_tree_wherever_the_search_view_is(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = setup(cx);
    let main = |vcx: &mut VisualTestContext| vcx.update(|window, cx| (a11y::windows_access::WORKBENCH.snapshot)(wb.clone().into(), window, cx)).unwrap();
    wb.update_in(vcx, |wb, window, cx| wb.find_in_files(&FindInFiles, window, cx));
    vcx.run_until_parked();
    assert_eq!(wb.read_with(vcx, |wb, _| wb.chrome.panel), Some(Panel::Search));
    assert!(main(vcx).views.search.is_some(), "primary side bar");
    wb.update(vcx, |wb, cx| wb.dock_view(Panel::Search, true, cx));
    vcx.run_until_parked();
    assert_eq!(wb.read_with(vcx, |wb, _| wb.chrome.secondary), Some(Panel::Search));
    assert!(main(vcx).views.search.is_some(), "secondary side bar");
    wb.update_in(vcx, |wb, window, cx| {
        wb.chrome.panel = None;
        wb.view_into_new_window(window, cx);
    });
    vcx.run_until_parked();
    assert!(wb.read_with(vcx, |wb, _| wb.in_view_window(Panel::Search)));
    assert!(main(vcx).views.search.is_none(), "not in the main window's tree");
    let handle = view_window(&wb, Panel::Search, vcx);
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    let root = handle.update(&mut win, |_, _, cx| cx.entity()).unwrap();
    let snap = win.update(|window, cx| (a11y::windows_access::VIEW_WINDOW.snapshot)(root.into(), window, cx)).unwrap();
    assert!(snap.views.search.is_some(), "its own window");
    accesskit_consumer::Tree::new(snap.to_tree(), true);
}

/// The view's content is drawn again only when what it shows changes: typing in an editor of the main window (which
/// changes the workbench) does not draw a Source Control window's view again.
#[gpui::test]
fn an_unrelated_change_does_not_redraw_a_view_window(cx: &mut gpui::TestAppContext) {
    let dir = fresh("viewwin-redraw");
    let file = dir.join("a.txt");
    std::fs::write(&file, "alpha\n").unwrap();
    let (wb, vcx) = setup(cx);
    open(&wb, &dir, vcx);
    crate::qa_workbench_tests::open_in(&wb, &file, vcx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.show_scm(&ShowScm, window, cx);
        wb.view_into_new_window(window, cx);
    });
    vcx.run_until_parked();
    let handle = view_window(&wb, Panel::Scm, vcx);
    let body = vcx.update(|_, cx| handle.read(cx).unwrap().body.clone());
    let renders = |vcx: &mut VisualTestContext| vcx.update(|_, cx| body.read(cx).renders);
    let before = renders(vcx);
    assert!(before > 0, "the view was drawn");
    wb.update_in(vcx, |wb, window, cx| {
        wb.leave_editor_window();
        wb.focus_group_key(wb.tabs.current_key(), window, cx);
    });
    for ch in ["x", "y", "z"] {
        vcx.simulate_input(ch);
        vcx.run_until_parked();
    }
    let ed = wb.read_with(vcx, |wb, _| wb.tab_editor().cloned()).unwrap();
    assert!(ed.read_with(vcx, |e, _| e.is_dirty()), "the edits happened");
    assert_eq!(renders(vcx), before, "typing in the main window did not draw the view again");
}

#[test]
fn saved_view_windows_are_versioned_and_bounded() {
    let saved = SavedViewWindows::parse(r#"{"version":1,"windows":[{"view":"scm","secondary":true,"at":[1,2,300,400]},{"view":"nope"},{"view":"scm"}]}"#)
        .expect("valid");
    assert_eq!(saved.windows.len(), 1, "unknown and repeated views are dropped");
    for bad in ["", "{", r#"{"version":2,"windows":[]}"#, r#"{"version":1,"windows":[{"view":"scm","x":1}]}"#] {
        assert!(SavedViewWindows::parse(bad).is_none(), "{bad}");
    }
    assert!(SavedViewWindows::parse(&" ".repeat(MAX_TEXT + 1)).is_none());
}
