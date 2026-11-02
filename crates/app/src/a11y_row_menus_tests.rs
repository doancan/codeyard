use accesskit::{Action, ActionRequest, Node, NodeId, Role, TreeId};
use gpui::{Entity, VisualTestContext, px, size};

use super::*;
use crate::qa_workbench_tests::setup;

fn nodes(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> (Snapshot, Vec<(NodeId, Node)>) {
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let nodes = snap.to_tree().nodes;
    (snap, nodes)
}

fn find(nodes: &[(NodeId, Node)], role: Role, label: &str) -> (NodeId, Node) {
    nodes.iter().find(|(_, n)| n.role() == role && n.label() == Some(label)).cloned().unwrap_or_else(|| panic!("{role:?} {label:?} is in the tree"))
}

fn act(wb: &Entity<Workbench>, snap: &Snapshot, node: NodeId, action: Action, cx: &mut VisualTestContext) {
    let req = ActionRequest { action, target_tree: TreeId::ROOT, target_node: node, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

/// The "More Actions…" button of `row`.
fn more(tree: &[(NodeId, Node)], row: NodeId) -> NodeId {
    let node = &tree.iter().find(|(id, _)| *id == row).unwrap().1;
    *node.children().iter().find(|id| tree.iter().any(|(i, n)| i == *id && n.label() == Some("More Actions…"))).expect("a More Actions… button")
}

fn menu(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<Vec<String>> {
    wb.read_with(cx, |wb, cx| wb.menu_labels(cx))
}

fn close(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.close_menu_for_test(window, cx));
    cx.run_until_parked();
}

/// AC1/AC6: a variable, a frame and a line breakpoint open the menu a right click opens, from the row
/// (`ShowContextMenu`) or its "More Actions…" button, under the row as painted; the menu takes the focus and its items
/// are read. A row that changed opens nothing.
#[gpui::test]
fn run_and_debug_rows_open_their_menu_without_a_pointer(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = setup(cx);
    cx.simulate_resize(size(px(1280.), px(800.)));
    wb.update(cx, |wb, cx| {
        wb.chrome.panel = Some(Panel::Debug);
        wb.dbg.bps.toggle_line("/p/src/main.rs", 2);
        wb.push_paused_session_for_test("fixture", cx);
    });
    cx.run_until_parked();
    let (snap, tree) = nodes(&wb, cx);
    let (point, node) = find(&tree, Role::TreeItem, "point");
    assert!(node.supports_action(Action::ShowContextMenu));
    let button = &tree.iter().find(|(id, _)| *id == more(&tree, point)).unwrap().1;
    assert_eq!((button.role(), button.description(), button.has_popup()), (Role::Button, Some("point"), Some(accesskit::HasPopup::Menu)));
    assert!(!find(&tree, Role::TreeItem, "Locals").1.supports_action(Action::ShowContextMenu), "a scope has no menu");

    act(&wb, &snap, point, Action::ShowContextMenu, cx);
    assert_eq!(menu(&wb, cx).as_deref(), Some(&["Set Value…", "Copy Value", "Copy as Expression", "Add to Watch"].map(String::from)[..]));
    let row =
        cx.update(|window, cx| wb.read(cx).chrome.a11y_bounds.row(window.window_handle().window_id(), RowFrame::DebugVariable, 1)).expect("the painted row");
    assert_eq!(wb.read_with(cx, |wb, _| wb.menu_position_for_test()), Some(row.bottom_left()), "under the row");
    let (open, items) = nodes(&wb, cx);
    assert!(items.iter().any(|(_, n)| n.role() == Role::MenuItem && n.label() == Some("Copy Value")));
    let consumer = accesskit_consumer::Tree::new(open.to_tree(), true);
    let focus = consumer.state().focus().map(|n| n.role());
    assert!(matches!(focus, Some(Role::Menu | Role::MenuItem)), "the menu has the focus: {focus:?}");
    close(&wb, cx);

    // The frame's menu through its button, the breakpoint's through the row.
    let (frame, _) = find(&tree, Role::TreeItem, "main");
    act(&wb, &snap, more(&tree, frame), Action::Click, cx);
    assert_eq!(menu(&wb, cx).as_deref(), Some(&["Restart Frame", "Copy Call Stack"].map(String::from)[..]));
    close(&wb, cx);
    let (bp, _) = find(&tree, Role::CheckBox, "main.rs, line 3");
    act(&wb, &snap, bp, Action::ShowContextMenu, cx);
    assert_eq!(menu(&wb, cx).as_deref(), Some(&["Edit Condition…", "Disable Breakpoint", "Remove Breakpoint"].map(String::from)[..]));
    close(&wb, cx);

    // The breakpoint was disabled meanwhile and another session became current: the old rows open nothing.
    wb.update(cx, |wb, cx| {
        wb.set_breakpoint_enabled("/p/src/main.rs".into(), 2, false, cx);
        wb.push_paused_session_for_test("second", cx);
    });
    act(&wb, &snap, bp, Action::ShowContextMenu, cx);
    act(&wb, &snap, frame, Action::ShowContextMenu, cx);
    act(&wb, &snap, point, Action::ShowContextMenu, cx);
    assert_eq!(menu(&wb, cx), None, "an outdated row opens no menu");
}

/// A host's "…" menu and a port row's menu: opened for the row the tree showed, never for one that changed.
#[gpui::test]
fn remote_hosts_and_ports_open_their_menu_without_a_pointer(cx: &mut gpui::TestAppContext) {
    use crate::port_forwards::{Effect, ForwardRequest};
    use host::forward::{LocalBind, Opened, RemoteTarget};

    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| {
        wb.remote.ssh_hosts =
            vec![host::ssh::SshHost { alias: "fixture".into(), hostname: Some("example.invalid".into()), user: Some("me".into()), source: None }];
        wb.chrome.panel = Some(Panel::Remote);
        cx.notify();
    });
    let (snap, tree) = nodes(&wb, cx);
    let (host, _) = find(&tree, Role::TreeItem, "fixture");
    act(&wb, &snap, more(&tree, host), Action::Click, cx);
    let labels = menu(&wb, cx).expect("the host's menu");
    assert_eq!(labels[..2], ["Connect in Current Window", "Connect in New Window"]);
    assert!(labels.contains(&"Edit Host…".to_string()));
    close(&wb, cx);
    wb.update(cx, |wb, _| wb.remote.ssh_hosts.clear());
    act(&wb, &snap, host, Action::ShowContextMenu, cx);
    assert_eq!(menu(&wb, cx), None, "the host is gone: no menu");

    wb.update(cx, |wb, cx| {
        wb.chrome.panel_open = true;
        wb.chrome.bottom = Bottom::Ports;
        wb.remote.conn = crate::workbench::Conn::Connected { alias: "fixture".into() };
        let (id, effects) = wb.ports.set.request(ForwardRequest::user("fixture", 8000, RemoteTarget::Localhost, LocalBind::Loopback));
        let Some(Effect::Open { seq, .. }) = effects.last() else { panic!("an open") };
        wb.ports.set.on_result(id, *seq, Ok(Opened { local: 8001, spec: "8001:localhost:8000".into() }));
        cx.notify();
    });
    let (snap, tree) = nodes(&wb, cx);
    let (row, node) = find(&tree, Role::ListItem, "Port 8000");
    assert!(node.supports_action(Action::ShowContextMenu));
    act(&wb, &snap, row, Action::ShowContextMenu, cx);
    let labels = menu(&wb, cx).expect("the port's menu");
    assert!(labels.contains(&"Stop Forwarding".to_string()) && labels.contains(&"Change Local Port…".to_string()), "{labels:?}");
    close(&wb, cx);
    // Stopped meanwhile: the row now shows another state, so its old menu request does nothing.
    wb.update(cx, |wb, cx| wb.stop_forward(8000, cx));
    act(&wb, &snap, more(&tree, row), Action::Click, cx);
    assert_eq!(menu(&wb, cx), None, "an outdated port row opens no menu");
}

/// AC3: rows of these views report the bounds they are painted at, inside the window; a hit test at a row's middle
/// finds that row, and a row scrolled out of its list has none.
#[gpui::test]
fn side_view_rows_have_painted_bounds(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = setup(cx);
    for width in [480., 1280.] {
        cx.simulate_resize(size(px(width), px(800.)));
        wb.update(cx, |wb, cx| {
            wb.chrome.panel = Some(Panel::Debug);
            if wb.dbg.sessions.is_empty() {
                wb.push_paused_session_for_test("fixture", cx);
            }
            cx.notify();
        });
        cx.run_until_parked();
        let (snap, tree) = nodes(&wb, cx);
        let root = tree.iter().find(|(id, _)| *id == ROOT).and_then(|(_, n)| n.bounds()).expect("window bounds");
        let consumer = accesskit_consumer::Tree::new(snap.to_tree(), true);
        for label in ["main", "point"] {
            let (_, row) = find(&tree, Role::TreeItem, label);
            let rect = row.bounds().unwrap_or_else(|| panic!("{label} has bounds at width {width}"));
            assert!(rect.width() > 0. && rect.height() > 0.);
            assert!(rect.x0 >= root.x0 && rect.y0 >= root.y0 && rect.x1 <= root.x1 && rect.y1 <= root.y1, "{label}: {rect:?} inside {root:?}");
            let center = accesskit::Point::new((rect.x0 + rect.x1) / 2., (rect.y0 + rect.y1) / 2.);
            let state = consumer.state();
            let hit = state.root().node_at_point(center, &|_| accesskit_consumer::FilterResult::Include).expect("a hit");
            assert_eq!((hit.role(), hit.label().as_deref()), (Role::TreeItem, Some(label)), "hit testing {label} at width {width}");
        }
    }
    // Another view in the side bar: the debug rows are no longer painted, so they keep no bounds.
    wb.update(cx, |wb, cx| {
        wb.chrome.panel = Some(Panel::Remote);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.update(|window, cx| wb.read(cx).chrome.a11y_bounds.row(window.window_handle().window_id(), RowFrame::DebugStack, 1)).is_none());
    let (_, tree) = nodes(&wb, cx);
    let (_, group) = find(&tree, Role::TreeItem, "SSH Hosts");
    assert!(group.bounds().is_some(), "a Remote Explorer row is painted");
}
