use accesskit::{Action, ActionRequest, Node, NodeId, Role, Toggled, TreeId};
use gpui::{Entity, VisualTestContext};

use super::*;
use crate::qa_workbench_tests::{open, project, setup};

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

/// The button `label` among the children of `row`.
fn child(tree: &[(NodeId, Node)], row: NodeId, label: &str) -> NodeId {
    let row = &tree.iter().find(|(id, _)| *id == row).unwrap().1;
    *row.children().iter().find(|id| tree.iter().any(|(i, n)| i == *id && n.label() == Some(label))).unwrap_or_else(|| panic!("{label}"))
}

#[gpui::test]
fn the_run_and_debug_view_reads_the_session_and_acts_only_on_what_it_showed(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| {
        wb.chrome.panel = Some(Panel::Debug);
        wb.dbg.bps.toggle_line("/p/src/main.rs", 2);
        wb.push_paused_session_for_test("fixture", cx);
    });
    let (snap, tree) = nodes(&wb, cx);
    let state = tree.iter().find(|(_, n)| n.role() == Role::Status && n.label().is_some_and(|l| l.starts_with("fixture: paused on"))).expect("the state");
    assert_eq!(state.1.live(), Some(accesskit::Live::Polite));
    let (point, node) = find(&tree, Role::TreeItem, "point");
    assert_eq!((node.value(), node.level(), node.is_expanded()), (Some("Point"), Some(2), Some(false)));
    assert_eq!(find(&tree, Role::TreeItem, "n").1.value(), Some("1"));
    assert!(!tree.iter().any(|(_, n)| n.label() == Some("x")), "a closed variable hides its children");
    let (frame, node) = find(&tree, Role::TreeItem, "main");
    assert_eq!((node.description(), node.is_selected()), (Some("main.rs:3"), Some(true)));
    let (_, bar) = find(&tree, Role::Toolbar, "Debug toolbar");
    assert_eq!(bar.children().len(), 6);
    assert!(find(&tree, Role::Button, "Continue (F5)").1.supports_action(Action::Click));
    let (bp, node) = find(&tree, Role::CheckBox, "main.rs, line 3");
    assert_eq!(node.toggled(), Some(Toggled::True));

    // Expanding the variable goes through the view's own toggle; the same request from the old tree changes nothing.
    act(&wb, &snap, point, Action::Expand, cx);
    let (_, tree) = nodes(&wb, cx);
    assert_eq!(find(&tree, Role::TreeItem, "point").1.is_expanded(), Some(true));
    assert_eq!(find(&tree, Role::TreeItem, "x").1.value(), Some("4"));
    act(&wb, &snap, point, Action::Click, cx);
    let (_, tree) = nodes(&wb, cx);
    assert_eq!(find(&tree, Role::TreeItem, "point").1.is_expanded(), Some(true), "an outdated row toggles nothing");

    // The breakpoint check box: unchecked like a click on it; a second click from the old tree is not repeated.
    act(&wb, &snap, bp, Action::Click, cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.breakpoints_for_test("/p/src/main.rs")), [(2, false, "?")]);
    act(&wb, &snap, bp, Action::Click, cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.breakpoints_for_test("/p/src/main.rs")), [(2, false, "?")], "an outdated toggle is ignored");

    // Another session becomes current: the old tree's Stop and frame act on nothing.
    let stop = find(&tree, Role::Button, "Stop (⇧F5)").0;
    wb.update(cx, |wb, cx| {
        wb.push_paused_session_for_test("second", cx);
    });
    act(&wb, &snap, stop, Action::Click, cx);
    act(&wb, &snap, frame, Action::Click, cx);
    assert!(wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0 == "stopped:breakpoint"), "an outdated Stop stops nothing");

    // The sessions are listed with their state; choosing one makes it current, as a click on its row does.
    let (snap, tree) = nodes(&wb, cx);
    let (first, node) = find(&tree, Role::TreeItem, "fixture");
    assert!(node.value().is_some_and(|v| v.starts_with("paused on")));
    act(&wb, &snap, first, Action::Click, cx);
    let (snap, tree) = nodes(&wb, cx);
    assert_eq!(find(&tree, Role::TreeItem, "fixture").1.is_selected(), Some(true));
    act(&wb, &snap, find(&tree, Role::Button, "Stop (⇧F5)").0, Action::Click, cx);
    let (_, tree) = nodes(&wb, cx);
    assert_eq!(find(&tree, Role::TreeItem, "fixture").1.value(), Some("ended"), "Stop stops the session it showed");
}

#[gpui::test]
fn the_testing_view_lists_tests_with_their_outcome_and_opens_only_a_listed_one(cx: &mut gpui::TestAppContext) {
    let dir = project("a11y-testing");
    let src = "pub fn two() -> i32 { 2 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn passes() {}\n\n    #[test]\n    fn fails() {}\n}\n";
    std::fs::write(dir.join("src/lib.rs"), src).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Testing", window, cx));
    crate::qa_integration_tests::wait_ticking(cx, "the tests to be found", |cx| wb.read_with(cx, |wb, _| !wb.testing_rows_for_test().is_empty()));
    let (snap, tree) = nodes(&wb, cx);
    assert!(tree.iter().any(|(_, n)| n.role() == Role::Status && n.label() == Some("0/2 passed")));
    assert!(find(&tree, Role::Button, "Run All Tests").1.supports_action(Action::Click));
    let (file, node) = find(&tree, Role::TreeItem, "lib.rs");
    assert_eq!((node.value(), node.is_expanded(), node.description()), (Some("not run"), Some(false), Some("src, 2 tests")));
    child(&tree, file, "Run Tests in File");

    act(&wb, &snap, file, Action::Expand, cx);
    act(&wb, &snap, file, Action::Expand, cx);
    let (_, tree) = nodes(&wb, cx);
    assert_eq!(find(&tree, Role::TreeItem, "lib.rs").1.is_expanded(), Some(true), "opened once; the outdated request does not close it");

    // A failed outcome is read with the test.
    wb.update(cx, |wb, cx| {
        let result = testexp::TestResult { outcome: Outcome::Failed, millis: Some(12), ..Default::default() };
        wb.jobs.tests.results.insert(("src/lib.rs".into(), wb.jobs.tests.files[0].tests[1].id.clone()), result);
        cx.notify();
    });
    let (snap, tree) = nodes(&wb, cx);
    let (fails, node) = find(&tree, Role::TreeItem, "fails");
    assert_eq!(node.value(), Some("failed, 12 ms"));
    child(&tree, fails, "Run Test");
    child(&tree, fails, "Debug Test");

    // The test is gone (renamed in the file): its old row opens nothing.
    let id = wb.update(cx, |wb, _| std::mem::replace(&mut wb.jobs.tests.files[0].tests[1].id, "renamed".into()));
    act(&wb, &snap, fails, Action::Click, cx);
    assert!(wb.read_with(cx, |wb, _| wb.active_editor().is_none()), "an outdated row opens nothing");
    wb.update(cx, |wb, _| wb.jobs.tests.files[0].tests[1].id = id);
    act(&wb, &snap, fails, Action::Click, cx);
    crate::qa_integration_tests::wait_ticking(cx, "the test to open", |cx| wb.read_with(cx, |wb, _| wb.active_editor().is_some()));
    let line = wb.read_with(cx, |wb, cx| wb.active_editor().unwrap().read(cx).cursor_position().0);
    assert_eq!(line, 9, "at the test's definition");
}

#[gpui::test]
fn the_remote_explorer_lists_hosts_with_their_state_and_ignores_an_outdated_connect(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| {
        wb.remote.ssh_hosts =
            vec![host::ssh::SshHost { alias: "fixture".into(), hostname: Some("example.invalid".into()), user: Some("me".into()), source: None }];
        wb.chrome.panel = Some(Panel::Remote);
        cx.notify();
    });
    let (snap, tree) = nodes(&wb, cx);
    let (group, node) = find(&tree, Role::TreeItem, "SSH Hosts");
    assert_eq!(node.is_expanded(), Some(true));
    let (host, node) = find(&tree, Role::TreeItem, "fixture");
    assert_eq!((node.value(), node.description()), (Some("Not connected yet"), Some("me@example.invalid")));
    let connect = child(&tree, host, "Connect in Current Window");
    child(&tree, host, "Connect in New Window");
    assert!(!tree.iter().any(|(_, n)| n.label() == Some("Disconnect")), "only a connected host offers Disconnect");

    // The host left the config: its old Connect button connects nothing.
    wb.update(cx, |wb, _| wb.remote.ssh_hosts.clear());
    act(&wb, &snap, connect, Action::Click, cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.conn_for_test()), "local");

    // Collapsing the group goes through the view's toggle.
    act(&wb, &snap, group, Action::Collapse, cx);
    let (_, tree) = nodes(&wb, cx);
    assert_eq!(find(&tree, Role::TreeItem, "SSH Hosts").1.is_expanded(), Some(false));
}

#[gpui::test]
fn an_open_menu_is_read_and_runs_its_item_but_not_from_another_menu(cx: &mut gpui::TestAppContext) {
    let dir = project("a11y-menu");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("README.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.open_tab_menu_for_test(0, window, cx));
    let (old, tree) = nodes(&wb, cx);
    let (_, menu) = find(&tree, Role::Menu, "Context menu");
    assert!(!menu.children().is_empty());
    let (close, node) = find(&tree, Role::MenuItem, "Close");
    assert!(node.supports_action(Action::Click));
    assert_eq!(node.position_in_set(), Some(1));
    assert_eq!(old.to_tree().focus, MENU, "the open menu has the focus");

    // Another menu replaced it: the old item does nothing and the new menu stays open.
    wb.update_in(cx, |wb, window, cx| {
        wb.close_menu_for_test(window, cx);
        wb.open_tab_menu_for_test(0, window, cx);
    });
    act(&wb, &old, close, Action::Click, cx);
    assert_eq!(wb.read_with(cx, |wb, cx| wb.open_paths(cx)).len(), 1, "an outdated item runs nothing");
    assert!(wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).is_some());

    // The current menu's item runs as a click on it does: the menu closes and the tab with it.
    let (snap, tree) = nodes(&wb, cx);
    act(&wb, &snap, find(&tree, Role::MenuItem, "Close").0, Action::Click, cx);
    assert!(wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).is_none());
    assert!(wb.read_with(cx, |wb, cx| wb.open_paths(cx)).is_empty());
    let (_, tree) = nodes(&wb, cx);
    assert!(!tree.iter().any(|(_, n)| n.role() == Role::Menu), "a closed menu leaves the tree");
}

/// The items of an open menu report where they are painted, inside the window, top to bottom.
#[gpui::test]
fn menu_items_have_their_painted_bounds(cx: &mut gpui::TestAppContext) {
    let dir = project("a11y-menu-bounds");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("README.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.open_tab_menu_for_test(0, window, cx));
    cx.run_until_parked();
    let (_, tree) = nodes(&wb, cx);
    // The open menu's items (on Windows and Linux the menu bar the app draws has menu items of its own).
    let menu = tree.iter().find(|(_, n)| n.role() == Role::Menu).map(|(_, n)| n.children().to_vec()).expect("the open menu");
    let items: Vec<&Node> =
        tree.iter().filter(|(id, n)| menu.contains(id) && matches!(n.role(), Role::MenuItem | Role::MenuItemCheckBox)).map(|(_, n)| n).collect();
    assert!(!items.is_empty());
    let (scale, size) = cx.update(|window, _| (f64::from(window.scale_factor()), window.viewport_size()));
    let (width, height) = (f64::from(size.width / px(1.)) * scale, f64::from(size.height / px(1.)) * scale);
    let mut last_bottom = 0.;
    for n in items {
        let b = n.bounds().unwrap_or_else(|| panic!("{:?} has bounds", n.label()));
        assert!(b.width() > 0. && b.height() > 0.);
        assert!(b.x1 <= width + 0.5 && b.y1 <= height + 0.5, "{b:?} inside the window");
        assert!(b.y0 >= last_bottom - 0.5, "items are drawn top to bottom: {b:?}");
        last_bottom = b.y1;
    }
}

/// A data breakpoint is a check box in the Breakpoints list like the others: its state, a toggle and a Remove button
/// that act only on what the tree showed, a row menu, and painted bounds.
#[gpui::test]
fn data_breakpoints_are_listed_toggled_and_removed_like_line_breakpoints(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| {
        wb.chrome.panel = Some(Panel::Debug);
        wb.push_paused_session_for_test("fixture", cx);
        let d = wb.dbg.cur_mut().unwrap();
        d.data_breakpoints.push(crate::debug_model::DataBp { id: "var:total".into(), desc: "total (4 bytes)".into(), enabled: true });
        cx.notify();
    });
    cx.run_until_parked();
    let (snap, tree) = nodes(&wb, cx);
    let (bp, node) = find(&tree, Role::CheckBox, "Data breakpoint: total (4 bytes)");
    assert_eq!(node.toggled(), Some(Toggled::True));
    assert!(node.description().is_some_and(|d| d.contains("debug session")), "it says it lasts for this session only");
    assert!(node.bounds().is_some_and(|b| b.height() > 0.), "the row has its painted bounds");
    assert!(tree.iter().any(|(id, n)| node.children().contains(id) && n.label() == Some(crate::i18n::t("More Actions…"))), "a row menu");

    // Unchecked like a click on its check box; the same click from the old tree is not repeated.
    act(&wb, &snap, bp, Action::Click, cx);
    let states = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.data_breakpoint_states_for_test());
    assert_eq!(states(cx), vec![("total (4 bytes)".to_string(), false)]);
    act(&wb, &snap, bp, Action::Click, cx);
    assert_eq!(states(cx), vec![("total (4 bytes)".to_string(), false)], "an outdated toggle is ignored");

    // Removed through its own button.
    let (snap, tree) = nodes(&wb, cx);
    let (bp, _) = find(&tree, Role::CheckBox, "Data breakpoint: total (4 bytes)");
    act(&wb, &snap, child(&tree, bp, crate::i18n::t("Remove Breakpoint")), Action::Click, cx);
    assert!(states(cx).is_empty());
}
