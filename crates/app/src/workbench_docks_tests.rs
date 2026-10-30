//! Docking: dragging a view's activity bar icon to the other side bar, and the panel below, left of or right of the
//! editor.

use super::*;
use crate::qa_workbench_tests::{fresh, open, setup, setup_persistent};
use gpui::{Modifiers, VisualTestContext, point};

/// Drags from the centre of `from` to the centre of the drop zone `to` (shown once the drag starts) and drops.
fn drag(cx: &mut VisualTestContext, from: &'static str, to: &'static str) {
    let start = cx.debug_bounds(from).unwrap_or_else(|| panic!("{from} is drawn")).center();
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(start + point(px(30.), px(4.)), Some(MouseButton::Left), Modifiers::none());
    cx.run_until_parked();
    let target = cx.debug_bounds(to).unwrap_or_else(|| panic!("the drop zone {to} is drawn while a view is dragged")).center();
    cx.simulate_mouse_move(target, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

/// AC1: a view's icon dragged to the secondary side opens it there (the primary side bar keeps its view), and dragged
/// back it returns; the drop zones exist only during the drag.
#[gpui::test]
fn dragging_a_views_icon_moves_it_between_the_side_bars(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = setup(cx);
    vcx.simulate_resize(gpui::size(px(1280.), px(800.)));
    wb.update_in(vcx, |wb, window, cx| wb.show_explorer(&ShowExplorer, window, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("secondary-drop-zone").is_none(), "no drop zones before a drag");
    drag(vcx, "act-scm", "secondary-drop-zone");
    wb.read_with(vcx, |wb, _| {
        assert_eq!(wb.chrome.in_secondary, vec![Panel::Scm]);
        assert_eq!((wb.chrome.panel, wb.chrome.secondary), (Some(Panel::Explorer), Some(Panel::Scm)), "both side bars show a view");
        assert!(wb.chrome.view_drag.is_none(), "the drag ended: the drop zones go");
    });
    assert!(vcx.debug_bounds("secondary-side-bar").is_some());
    drag(vcx, "act-scm", "primary-drop-zone");
    wb.read_with(vcx, |wb, _| {
        assert!(wb.chrome.in_secondary.is_empty());
        assert_eq!((wb.chrome.panel, wb.chrome.secondary), (Some(Panel::Scm), None));
    });
    // A drag dropped elsewhere moves nothing and takes the zones away.
    let start = vcx.debug_bounds("act-search").unwrap().center();
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(start + point(px(30.), px(4.)), Some(MouseButton::Left), Modifiers::none());
    vcx.run_until_parked();
    let middle = point(px(640.), px(300.));
    vcx.simulate_mouse_move(middle, Some(MouseButton::Left), Modifiers::none());
    vcx.simulate_mouse_up(middle, MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    wb.read_with(vcx, |wb, _| assert!(wb.chrome.in_secondary.is_empty() && wb.chrome.view_drag.is_none()));
}

/// The panel docks left of, right of and below the editor, by command; the dock is saved and comes back.
#[gpui::test]
fn the_panel_docks_left_right_and_bottom_and_the_dock_is_saved(cx: &mut gpui::TestAppContext) {
    let home = fresh("dock-panel-home");
    let dir = fresh("dock-panel-project");
    let (wb, vcx) = setup_persistent(cx, &home);
    vcx.simulate_resize(gpui::size(px(1280.), px(800.)));
    open(&wb, &dir, vcx);
    let place = |vcx: &mut VisualTestContext| (vcx.debug_bounds("panel").expect("the panel is drawn"), vcx.debug_bounds("editor-area").unwrap());
    wb.update_in(vcx, |_, window, cx| window.dispatch_action(Box::new(MovePanelLeft), cx));
    vcx.run_until_parked();
    let (panel, editor) = place(vcx);
    assert!(panel.right() <= editor.left() + px(1.), "left of the editor: {panel:?} {editor:?}");
    assert_eq!(wb.read_with(vcx, |wb, _| wb.chrome.panel_dock), docks_glue::PanelDock::Left);
    wb.update_in(vcx, |_, window, cx| window.dispatch_action(Box::new(MovePanelRight), cx));
    vcx.run_until_parked();
    let (panel, editor) = place(vcx);
    assert!(panel.left() >= editor.right() - px(1.), "right of the editor: {panel:?} {editor:?}");
    wb.update_in(vcx, |_, window, cx| window.dispatch_action(Box::new(MovePanelLeft), cx));
    vcx.run_until_parked();
    let state = home.join("state.db");
    assert_eq!(state::State::open(&state).unwrap().get("panel_dock").as_deref(), Some("left"));
    // A new window reads the saved dock.
    assert_eq!(docks_glue::PanelDock::load(state::State::open(&state).ok().as_ref()), docks_glue::PanelDock::Left);
    wb.update_in(vcx, |_, window, cx| window.dispatch_action(Box::new(MovePanelToBottom), cx));
    vcx.run_until_parked();
    let (panel, editor) = place(vcx);
    assert!(panel.top() >= editor.bottom() - px(1.), "below the editor: {panel:?} {editor:?}");
}

/// Docked left, the panel's edge towards the editor resizes it.
#[gpui::test]
fn a_panel_docked_left_resizes_from_its_right_edge(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = setup(cx);
    vcx.simulate_resize(gpui::size(px(1280.), px(800.)));
    wb.update(vcx, |wb, cx| wb.dock_panel(docks_glue::PanelDock::Left, cx));
    vcx.run_until_parked();
    let before = wb.read_with(vcx, |wb, _| wb.chrome.panel_w);
    let sash = vcx.debug_bounds("panel").unwrap();
    let at = point(sash.right() - px(1.), sash.center().y);
    vcx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(at + point(px(60.), px(0.)), Some(MouseButton::Left), Modifiers::none());
    vcx.simulate_mouse_up(at + point(px(60.), px(0.)), MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    let after = wb.read_with(vcx, |wb, _| wb.chrome.panel_w);
    assert!((after - before - 60.).abs() < 3., "grew by the drag: {before} -> {after}");
}

/// An older saved layout (bottom ⇄ right only) still docks right; an unknown value docks at the bottom.
#[test]
fn the_saved_dock_reads_the_older_setting_and_ignores_damage() {
    let s = state::State::in_memory().unwrap();
    assert_eq!(docks_glue::PanelDock::load(Some(&s)), docks_glue::PanelDock::Bottom);
    s.set("panel_right", "1").unwrap();
    assert_eq!(docks_glue::PanelDock::load(Some(&s)), docks_glue::PanelDock::Right);
    s.set("panel_dock", "sideways").unwrap();
    assert_eq!(docks_glue::PanelDock::load(Some(&s)), docks_glue::PanelDock::Right, "damage falls back to the older setting");
    s.set("panel_dock", "left").unwrap();
    assert_eq!(docks_glue::PanelDock::load(Some(&s)), docks_glue::PanelDock::Left);
}

/// AC8: screen readers read where the views are: the window's description names each side bar's view and the dock.
#[gpui::test]
fn the_accessibility_tree_says_where_the_views_are(cx: &mut gpui::TestAppContext) {
    let (wb, vcx) = setup(cx);
    wb.update_in(vcx, |wb, window, cx| {
        wb.show_explorer(&ShowExplorer, window, cx);
        wb.dock_view(Panel::Scm, true, cx);
        wb.dock_panel(docks_glue::PanelDock::Left, cx);
    });
    vcx.run_until_parked();
    let snap = wb.update_in(vcx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
    let description = tree.state().root().description().unwrap_or_default();
    for part in ["Primary side bar: Explorer", "Secondary side bar: Source Control", "Panel: left of the editor"] {
        assert!(description.contains(part), "{part:?} in {description:?}");
    }
}
