//! Keyboard rows: reaching the lists with Tab, moving with the arrows, Enter as a click, Shift+F10 and the Menu key for
//! the row's menu with the focus coming back on Escape, and a cursor that only ever names a row of the list as it is now.

use gpui::{Entity, VisualTestContext, px, size};

use super::*;
use crate::qa_workbench_tests::{open, open_in, project, setup};

fn keys(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    cx.run_until_parked();
}

fn focused(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<(RowFrame, usize)> {
    wb.update_in(cx, |wb, window, cx| wb.focused_row(window, cx))
}

fn menu(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<Vec<String>> {
    wb.read_with(cx, |wb, cx| wb.menu_labels(cx))
}

fn focus_list(wb: &Entity<Workbench>, kind: RowFrame, ix: usize, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.focus_row_list_for_test(kind, ix, window, cx));
    cx.run_until_parked();
}

/// The stack rows as the view shows them ("frame:main" and so on).
fn stack(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    wb.read_with(cx, |wb, _| wb.run_debug_rows_for_test().1)
}

#[gpui::test]
fn run_and_debug_rows_are_driven_by_the_keyboard(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = setup(cx);
    cx.simulate_resize(size(px(1280.), px(800.)));
    wb.update(cx, |wb, cx| {
        wb.chrome.panel = Some(Panel::Debug);
        wb.dbg.bps.toggle_line("/p/src/main.rs", 2);
        wb.push_paused_session_for_test("fixture", cx);
    });
    cx.run_until_parked();
    let rows = stack(&wb, cx);
    let start = rows.iter().position(|r| r.contains("start")).expect("the second frame");

    // Arrows, Home and End move the cursor.
    focus_list(&wb, RowFrame::DebugStack, 0, cx);
    keys(cx, "end");
    assert_eq!(focused(&wb, cx), Some((RowFrame::DebugStack, rows.len() - 1)));
    keys(cx, "home");
    assert_eq!(focused(&wb, cx), Some((RowFrame::DebugStack, 0)));
    for _ in 0..start {
        keys(cx, "down");
    }
    keys(cx, "up down");
    assert_eq!(focused(&wb, cx), Some((RowFrame::DebugStack, start)));

    // Shift+F10 opens the frame's menu under the row; the menu has the keys, Escape gives them back to the list.
    keys(cx, "shift-f10");
    assert_eq!(menu(&wb, cx).as_deref(), Some(&["Restart Frame", "Copy Call Stack"].map(String::from)[..]));
    let row = wb.read_with(cx, |wb, _| wb.chrome.a11y_bounds.row(RowFrame::DebugStack, start)).expect("the painted row");
    assert_eq!(wb.read_with(cx, |wb, _| wb.menu_position_for_test()), Some(row.bottom_left()));
    keys(cx, "down");
    assert_eq!(focused(&wb, cx), None, "the menu has the keyboard, not the list");
    keys(cx, "escape");
    assert_eq!(menu(&wb, cx), None);
    assert_eq!(focused(&wb, cx), Some((RowFrame::DebugStack, start)), "focus is back on the row");
    let tree = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx)).to_tree();
    let consumer = accesskit_consumer::Tree::new(tree, true);
    let focus = consumer.state().focus().map(|n| (n.role(), n.label()));
    assert_eq!(focus, Some((accesskit::Role::TreeItem, Some("start".into()))), "a screen reader follows the cursor row");

    // Variables: Enter opens a value with children, as a click does, and Shift+F10 opens its menu.
    focus_list(&wb, RowFrame::DebugVariable, 1, cx);
    keys(cx, "enter");
    assert!(wb.read_with(cx, |wb, _| wb.run_debug_rows_for_test().0).iter().any(|r| r.trim_start().starts_with("x:")), "point opened");
    keys(cx, "shift-f10");
    assert_eq!(menu(&wb, cx).map(|m| m[0].clone()).as_deref(), Some("Set Value…"));
    keys(cx, "escape");

    // Breakpoints: the Menu key works as Shift+F10 does.
    focus_list(&wb, RowFrame::DebugBreakpoint, 0, cx);
    keys(cx, "menu");
    assert_eq!(menu(&wb, cx).map(|m| m[0].clone()).as_deref(), Some("Edit Condition…"));
    keys(cx, "escape");
    assert_eq!(focused(&wb, cx), Some((RowFrame::DebugBreakpoint, 0)));

    // The rows change under the cursor (another session lists its sessions first): the menu key acts on the row the
    // cursor is on now, and a row without a menu opens nothing.
    focus_list(&wb, RowFrame::DebugStack, 0, cx);
    wb.update(cx, |wb, cx| {
        wb.push_paused_session_for_test("second", cx);
    });
    cx.run_until_parked();
    let now = stack(&wb, cx);
    assert!(!now[0].starts_with("frame"), "{now:?}");
    keys(cx, "shift-f10");
    assert_eq!(menu(&wb, cx), None, "a session row has no menu");
    // Every session ends and the stack empties: the keys do nothing at all.
    wb.update(cx, |wb, cx| {
        wb.dbg.sessions.clear();
        cx.notify();
    });
    cx.run_until_parked();
    keys(cx, "down enter shift-f10");
    assert_eq!(menu(&wb, cx), None);
}

#[gpui::test]
fn ports_rows_are_reached_with_tab_and_open_their_menu_by_keyboard(cx: &mut gpui::TestAppContext) {
    use crate::port_forwards::{Effect, ForwardRequest};
    use host::forward::{FailKind, ForwardError, LocalBind, Opened, RemoteTarget};

    let (wb, cx) = setup(cx);
    cx.simulate_resize(size(px(1280.), px(800.)));
    wb.update(cx, |wb, cx| {
        wb.chrome.panel_open = true;
        wb.chrome.bottom = Bottom::Ports;
        wb.remote.conn = crate::workbench::Conn::Connected { alias: "fixture".into() };
        for (port, local) in [(8000, Some(8001)), (3000, None)] {
            let (id, effects) = wb.ports.set.request(ForwardRequest::user("fixture", port, RemoteTarget::Localhost, LocalBind::Loopback));
            let Some(Effect::Open { seq, .. }) = effects.last() else { panic!("an open") };
            let result = match local {
                Some(local) => Ok(Opened { local, spec: format!("{local}:localhost:{port}") }),
                None => Err(ForwardError { kind: FailKind::AddrInUse, message: "in use".into() }),
            };
            wb.ports.set.on_result(id, *seq, result);
        }
        cx.notify();
    });
    cx.run_until_parked();

    // Tab reaches the list.
    for _ in 0..200 {
        if focused(&wb, cx).is_some_and(|(k, _)| k == RowFrame::Port) {
            break;
        }
        cx.update(|window, _| window.focus_next());
        cx.run_until_parked();
    }
    assert_eq!(focused(&wb, cx), Some((RowFrame::Port, 0)), "Tab reaches the Ports list");
    let labels = port_labels(&wb, cx);
    let failed = labels.iter().position(|l| l.contains("3000")).expect("the failed forward");

    keys(cx, "end");
    assert_eq!(focused(&wb, cx), Some((RowFrame::Port, labels.len() - 1)));
    keys(cx, "home");
    for _ in 0..failed {
        keys(cx, "down");
    }
    keys(cx, "shift-f10");
    let items = menu(&wb, cx).expect("the row's menu");
    assert!(items.contains(&"Retry".to_string()) && items.contains(&"Stop Forwarding".to_string()), "{items:?}");
    keys(cx, "escape");
    assert_eq!(focused(&wb, cx), Some((RowFrame::Port, failed)), "focus is back on the row");

    // The row under the cursor is stopped: the cursor stays inside the list and names the row now there.
    wb.update(cx, |wb, cx| {
        wb.stop_forward(3000, cx);
        cx.notify();
    });
    cx.run_until_parked();
    let left = port_labels(&wb, cx);
    assert_eq!(left.len(), labels.len() - 1);
    keys(cx, "shift-f10");
    let items = menu(&wb, cx).expect("the remaining row's menu");
    assert!(items.contains(&"Change Local Port…".to_string()), "{items:?}");
    keys(cx, "escape");
    wb.update(cx, |wb, cx| {
        wb.stop_forward(8000, cx);
        cx.notify();
    });
    cx.run_until_parked();
    keys(cx, "up shift-f10");
    assert_eq!(menu(&wb, cx), None, "an empty list opens nothing");
}

/// The same key opens the Explorer's and the editor's menus, so it works the same everywhere.
#[gpui::test]
fn the_context_menu_key_works_in_the_explorer_and_the_editor(cx: &mut gpui::TestAppContext) {
    let dir = project("row-nav-menu-key");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| {
        let tree = wb.roots.primary.clone();
        window.focus(&tree.focus_handle(cx));
    });
    keys(cx, "down shift-f10");
    let items = menu(&wb, cx).expect("the Explorer row's menu");
    assert!(items.iter().any(|l| l.starts_with("New File")), "{items:?}");
    keys(cx, "escape");
    assert!(wb.update_in(cx, |wb, window, cx| wb.roots.primary.focus_handle(cx).is_focused(window)), "back in the Explorer");

    open_in(&wb, &dir.join("src/main.rs"), cx);
    keys(cx, "shift-f10");
    let items = menu(&wb, cx).expect("the editor's menu");
    assert!(items.iter().any(|l| l == "Cut" || l == "Copy"), "{items:?}");
    keys(cx, "escape");
    assert!(wb.update_in(cx, |wb, window, cx| wb.active_editor().is_some_and(|e| e.read(cx).focus_handle(cx).is_focused(window))), "back in the editor");
}

/// The rows of the Ports list as it draws them (server port first).
fn port_labels(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    wb.read_with(cx, |wb, _| {
        let alias = wb.ssh_alias().unwrap_or_default();
        crate::workbench::ports_glue::port_rows(&wb.ports.set, &wb.ports.listening, &alias).iter().map(|r| r.port.to_string()).collect()
    })
}

/// A data breakpoint row by keyboard: Enter turns it off and on (as its check box does), Shift+F10 opens its menu.
#[gpui::test]
fn data_breakpoint_rows_are_driven_by_the_keyboard(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = setup(cx);
    cx.simulate_resize(size(px(1280.), px(800.)));
    wb.update(cx, |wb, cx| {
        wb.chrome.panel = Some(Panel::Debug);
        wb.dbg.bps.toggle_line("/p/src/main.rs", 2);
        wb.push_paused_session_for_test("fixture", cx);
        let d = wb.dbg.cur_mut().unwrap();
        d.data_breakpoints.push(crate::debug_model::DataBp { id: "var:total".into(), desc: "total".into(), enabled: true });
    });
    cx.run_until_parked();
    let states = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.data_breakpoint_states_for_test());
    focus_list(&wb, RowFrame::DebugBreakpoint, 0, cx);
    keys(cx, "enter");
    assert_eq!(states(cx), vec![("total".to_string(), false)], "Enter turned it off");
    keys(cx, "enter");
    assert_eq!(states(cx), vec![("total".to_string(), true)]);
    keys(cx, "shift-f10");
    assert_eq!(menu(&wb, cx).as_deref(), Some(&["Disable Breakpoint", "Remove Breakpoint"].map(String::from)[..]));
    keys(cx, "escape");
    // The line breakpoint follows it in the list.
    keys(cx, "down");
    assert_eq!(focused(&wb, cx), Some((RowFrame::DebugBreakpoint, 1)));
}
