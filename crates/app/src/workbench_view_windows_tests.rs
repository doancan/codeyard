//! Side views in windows of their own: the same view moved out and back, commands and menus there, restarts and the
//! accessibility trees.

use super::*;
use crate::qa_workbench_tests::{fresh, open, setup, setup_persistent};
use gpui::VisualTestContext;

fn window_count(cx: &mut VisualTestContext) -> usize {
    cx.update(|_, cx| cx.windows().len())
}

fn view_window(wb: &Entity<Workbench>, panel: Panel, cx: &mut VisualTestContext) -> gpui::WindowHandle<ViewWindow> {
    wb.read_with(cx, |wb, _| wb.chrome.view_windows.iter().find(|w| w.view.panel == panel).map(|w| w.window)).expect("the view has a window")
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
    assert_eq!(handle.update(&mut win, |root, _, _| root.view.panel).unwrap(), Panel::Scm, "the window draws Source Control");
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
        assert!(wb.chrome.view_host == wb.view_window_of(Panel::Scm), "the view's window is the one in use");
        assert!(wb.chrome.view_host.is_some());
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
        wb.use_view_window(handle.into(), cx);
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

/// A view window holds its workbench weakly: once the main window and the view window are gone, the workbench is
/// released (and, with it, everything it owns).
#[gpui::test]
fn closing_the_windows_releases_the_workbench(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = setup(cx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.show_scm(&ShowScm, window, cx);
        wb.view_into_new_window(window, cx);
        wb.show_explorer(&ShowExplorer, window, cx);
        wb.view_into_new_window(window, cx);
    });
    vcx.run_until_parked();
    let handles: Vec<gpui::AnyWindowHandle> = wb.read_with(vcx, |wb, _| wb.chrome.view_windows.iter().map(|w| w.window.into()).collect());
    assert_eq!(handles.len(), 2);
    let weak = wb.downgrade();
    drop(wb);
    vcx.update(|window, _| window.remove_window());
    let app: &mut gpui::TestAppContext = vcx;
    app.run_until_parked();
    // Closing the main window alone releases the workbench, which closes its view windows: nothing in them keeps it.
    assert!(weak.upgrade().is_none(), "the workbench was released with its window");
    assert!(app.update(|cx| cx.windows().is_empty()), "its view windows closed with it");
    for handle in handles {
        // Closing one that is already gone is harmless.
        app.update(|cx| handle.update(cx, |_, window, _| window.remove_window()).ok());
    }
    app.run_until_parked();
    assert!(weak.upgrade().is_none(), "the workbench was released");
}

/// An extension's side bar view moves into a window of its own as the same hosted view: the extension's new trees
/// reach the window, a screen reader's click there goes to the extension, the window's tree holds the view (the main
/// window's does not), and the view goes back to the side bar with its tree.
#[gpui::test]
fn an_extension_view_moves_into_its_own_window_with_its_state(cx: &mut gpui::TestAppContext) {
    use crate::ext_views_tests as probe;
    let (wb, vcx) = probe::setup(cx, "extwin-move", true);
    probe::open(&wb, vcx, probe::SIDE);
    probe::render(&wb, vcx, probe::SIDE, &probe::column([probe::text("one"), probe::button("go", "Go")]));
    wb.update_in(vcx, |wb, window, cx| wb.view_into_new_window(window, cx));
    vcx.run_until_parked();
    let handle = wb.read_with(vcx, |wb, _| {
        assert_eq!(wb.ext_views_in_windows(), vec![probe::SIDE.to_string()]);
        assert_eq!((wb.chrome.panel, wb.cfg.exts.views.sidebar.clone()), (None, None), "the side bar let go of it");
        assert!(wb.ext_view(probe::SIDE).is_some_and(|h| h.tree.is_some()), "the same hosted view, with its tree");
        wb.chrome.view_windows[0].window
    });
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    let root = handle.update(&mut win, |_, _, cx| cx.entity()).unwrap();
    let snapshot =
        |win: &mut VisualTestContext| win.update(|window, cx| (a11y::windows_access::VIEW_WINDOW.snapshot)(root.clone().into(), window, cx)).unwrap();
    // The extension draws again: the window shows the new tree.
    probe::render(&wb, vcx, probe::SIDE, &probe::column([probe::text("two"), probe::button("go", "Go")]));
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    let snap = snapshot(&mut win);
    let labels: Vec<String> = snap.ext_views.iter().flat_map(|v| v.nodes.iter().map(|n| n.label.clone())).collect();
    assert!(labels.iter().any(|l| l == "two"), "the extension's update reached the window: {labels:?}");
    let main = vcx.update(|window, cx| (a11y::windows_access::WORKBENCH.snapshot)(wb.clone().into(), window, cx)).unwrap();
    assert!(main.ext_views.is_empty(), "the main window's tree leaves it out");
    accesskit_consumer::Tree::new(snap.to_tree(), true);
    // A screen reader clicks the button in the window: the extension hears it.
    let i = snap.ext_views[0].nodes.iter().position(|n| n.label == "Go").expect("the button is in the tree");
    let req = accesskit::ActionRequest {
        action: accesskit::Action::Click,
        target_tree: accesskit::TreeId::ROOT,
        target_node: a11y::windows_access::ext_view_node(0, i),
        data: None,
    };
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    win.update(|window, cx| (a11y::windows_access::VIEW_WINDOW.action)(root.clone().into(), &req, &snap, window, cx));
    vcx.run_until_parked();
    let events = wb.read_with(vcx, |wb, _| wb.ext_view_events_for_test());
    assert!(events.iter().any(|e| e.contains("click") && e.contains("go")), "{events:?}");
    // Back to the side bar, with its tree.
    wb.update_in(vcx, |wb, _, cx| wb.return_view_windows(cx));
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert!(wb.ext_views_in_windows().is_empty());
        assert_eq!((wb.chrome.panel, wb.cfg.exts.views.sidebar.as_deref()), (Some(Panel::ExtView), Some(probe::SIDE)));
        assert!(wb.ext_view(probe::SIDE).is_some_and(|h| h.tree.is_some()));
    });
}

/// An extension view also moves to the secondary side bar (by command), where a screen reader finds it; from there it
/// moves into a window and back to the secondary side bar.
#[gpui::test]
fn an_extension_view_moves_to_the_secondary_side_bar_and_back_from_its_window(cx: &mut gpui::TestAppContext) {
    use crate::ext_views_tests as probe;
    let (wb, vcx) = probe::setup(cx, "extwin-secondary", true);
    probe::open(&wb, vcx, probe::SIDE);
    probe::render(&wb, vcx, probe::SIDE, &probe::column([probe::text("side")]));
    wb.update(vcx, |wb, cx| wb.move_view_to_secondary(cx));
    vcx.run_until_parked();
    assert_eq!(wb.read_with(vcx, |wb, _| wb.chrome.secondary), Some(Panel::ExtView));
    let main = vcx.update(|window, cx| (a11y::windows_access::WORKBENCH.snapshot)(wb.clone().into(), window, cx)).unwrap();
    assert_eq!(main.ext_views.len(), 1, "in the secondary side bar, a screen reader finds it");
    wb.update_in(vcx, |wb, window, cx| wb.view_into_new_window(window, cx));
    vcx.run_until_parked();
    assert_eq!(wb.read_with(vcx, |wb, _| wb.ext_views_in_windows()), vec![probe::SIDE.to_string()]);
    wb.update_in(vcx, |wb, _, cx| wb.return_view_windows(cx));
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert_eq!(wb.chrome.secondary, Some(Panel::ExtView), "back in the secondary side bar");
        assert_eq!(wb.cfg.exts.views.sidebar.as_deref(), Some(probe::SIDE));
    });
}

/// The window closes when the extension is gone, gives the view back when its host crashed, and a saved window comes
/// back only for an extension that still declares the view.
#[gpui::test]
fn extension_view_windows_follow_their_extension(cx: &mut gpui::TestAppContext) {
    use crate::ext_views_tests as probe;
    let (wb, vcx) = probe::setup(cx, "extwin-gone", true);
    probe::open(&wb, vcx, probe::SIDE);
    probe::render(&wb, vcx, probe::SIDE, &probe::column([probe::text("x")]));
    wb.update_in(vcx, |wb, window, cx| wb.view_into_new_window(window, cx));
    vcx.run_until_parked();
    // Its host stopped after a crash: back to the side bar.
    wb.update(vcx, |wb, cx| wb.return_ext_view_windows("you.probe", cx));
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| {
        assert!(wb.ext_views_in_windows().is_empty());
        assert_eq!(wb.cfg.exts.views.sidebar.as_deref(), Some(probe::SIDE));
    });
    // Uninstalled or turned off: the window closes.
    let windows = vcx.update(|_, cx| cx.windows().len());
    wb.update_in(vcx, |wb, window, cx| wb.view_into_new_window(window, cx));
    vcx.run_until_parked();
    wb.update(vcx, |wb, cx| wb.close_gone_ext_view_windows(&std::collections::HashSet::new(), cx));
    vcx.run_until_parked();
    assert!(wb.read_with(vcx, |wb, _| wb.ext_views_in_windows().is_empty()));
    assert_eq!(vcx.update(|_, cx| cx.windows().len()), windows, "its window closed");
    // Restoring: a view that is no longer declared is dropped, a declared one comes back with its extension view.
    let gone = SavedViewWindows::parse(r#"{"version":1,"windows":[{"view":"extension-view","ext":"you.gone.view"}]}"#);
    wb.update_in(vcx, |wb, window, cx| wb.restore_view_windows(gone, window, cx));
    vcx.run_until_parked();
    assert!(wb.read_with(vcx, |wb, _| wb.chrome.view_windows.is_empty()), "dropped cleanly");
    let saved = SavedViewWindows::parse(&format!(r#"{{"version":1,"windows":[{{"view":"extension-view","ext":"{}"}}]}}"#, probe::SIDE));
    wb.update_in(vcx, |wb, window, cx| wb.restore_view_windows(saved, window, cx));
    vcx.run_until_parked();
    assert_eq!(wb.read_with(vcx, |wb, _| wb.ext_views_in_windows()), vec![probe::SIDE.to_string()]);
}

#[test]
fn saved_view_windows_are_versioned_and_bounded() {
    let saved = SavedViewWindows::parse(r#"{"version":1,"windows":[{"view":"scm","secondary":true,"at":[1,2,300,400]},{"view":"nope"},{"view":"scm"}]}"#)
        .expect("valid");
    assert_eq!(saved.windows.len(), 1, "unknown and repeated views are dropped");
    let ext = SavedViewWindows::parse(
        r#"{"version":1,"windows":[{"view":"extension-view","ext":"a.b.c"},{"view":"extension-view","ext":"a.b.d"},{"view":"extension-view","ext":"a.b.c"},{"view":"extension-view"},{"view":"scm","ext":"a.b.c"}]}"#,
    )
    .expect("valid");
    assert_eq!(ext.windows.len(), 2, "two extension views; a repeat, one without an id and a built-in view with one are dropped");
    for bad in ["", "{", r#"{"version":2,"windows":[]}"#, r#"{"version":1,"windows":[{"view":"scm","x":1}]}"#] {
        assert!(SavedViewWindows::parse(bad).is_none(), "{bad}");
    }
    assert!(SavedViewWindows::parse(&" ".repeat(MAX_TEXT + 1)).is_none());
}
