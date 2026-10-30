use accesskit::{Action, ActionData, ActionRequest, Node, NodeId, Role, Toggled, TreeId};
use gpui::{Entity, VisualTestContext};

use super::*;
use crate::qa_workbench_tests::{open, project, setup, setup_with_config};

fn nodes(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> (Snapshot, Vec<(NodeId, Node)>) {
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let nodes = snap.to_tree().nodes;
    (snap, nodes)
}

fn find(nodes: &[(NodeId, Node)], role: Role, label: &str) -> (NodeId, Node) {
    nodes.iter().find(|(_, n)| n.role() == role && n.label() == Some(label)).cloned().unwrap_or_else(|| panic!("{role:?} {label:?} is in the tree"))
}

fn act(wb: &Entity<Workbench>, snap: &Snapshot, node: NodeId, action: Action, data: Option<ActionData>, cx: &mut VisualTestContext) {
    let req = ActionRequest { action, target_tree: TreeId::ROOT, target_node: node, data };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

fn minimap(cx: &mut VisualTestContext) -> bool {
    cx.read_global(|s: &crate::settings::Settings, _| s.minimap)
}

#[gpui::test]
fn settings_rows_show_their_values_and_change_like_the_pointer_does(cx: &mut gpui::TestAppContext) {
    let cfg = std::env::temp_dir().join(format!("codeyard-config-a11y-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cfg);
    std::fs::create_dir_all(&cfg).unwrap();
    let dir = project("a11y-settings");
    let (wb, cx) = setup_with_config(cx, &cfg);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Open Settings", window, cx));
    let (snap, tree) = nodes(&wb, cx);
    assert_eq!(snap.to_tree().focus, SETTINGS_SEARCH, "the page opens with its search field focused");
    assert_eq!(find(&tree, Role::Tab, "Commonly Used").1.is_selected(), Some(true));
    let (theme, node) = find(&tree, Role::Button, "Theme");
    assert_eq!(node.value(), Some("Auto (follow system)"));
    assert!(node.supports_action(Action::Click), "{theme:?} opens the theme picker");
    let (wrap, node) = find(&tree, Role::RadioGroup, "Editor: Word Wrap");
    assert!(node.description().is_some_and(|d| !d.is_empty()), "the description is read with the title");
    assert!(!node.children().is_empty(), "{wrap:?} has one button per option");

    // A switch: its state, and a click writes the opposite value through the page, as a click on the switch does.
    let before = minimap(cx);
    let (switch, node) = find(&tree, Role::CheckBox, "Editor › Minimap: Enabled");
    assert_eq!(node.toggled(), Some(Toggled::from(before)));
    act(&wb, &snap, switch, Action::Click, None, cx);
    assert_eq!(minimap(cx), !before);
    assert!(std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap().contains("\"editor.minimap.enabled\""));
    // The same click from the old tree (it showed the old state) changes nothing.
    act(&wb, &snap, switch, Action::Click, None, cx);
    assert_eq!(minimap(cx), !before, "an outdated toggle is ignored");
    let (snap, tree) = nodes(&wb, cx);
    let (_, node) = find(&tree, Role::CheckBox, "Editor › Minimap: Enabled");
    assert_eq!(node.toggled(), Some(Toggled::from(!before)));
    let (reset, _) =
        tree.iter().find(|(_, n)| n.role() == Role::Button && n.label() == Some("Reset")).cloned().expect("a Reset button for the changed setting");
    act(&wb, &snap, reset, Action::Click, None, cx);
    assert_eq!(minimap(cx), before, "Reset goes back to the default");

    // A number field: the value is checked as Enter in the field checks it; the error is read with the field.
    let (snap, tree) = nodes(&wb, cx);
    let (size, _) = find(&tree, Role::TextInput, "Editor: Font Size");
    act(&wb, &snap, size, Action::SetValue, Some(ActionData::Value("99".into())), cx);
    let (snap, tree) = nodes(&wb, cx);
    let (_, node) = find(&tree, Role::TextInput, "Editor: Font Size");
    assert_eq!(node.value(), Some("99"));
    assert_eq!(node.invalid(), Some(accesskit::Invalid::True));
    act(&wb, &snap, size, Action::SetValue, Some(ActionData::Value("15".into())), cx);
    assert_eq!(cx.read_global(|s: &crate::settings::Settings, _| s.editor_font_size), 15.);

    // Categories and the search field.
    let (snap, tree) = nodes(&wb, cx);
    let (editor, _) = find(&tree, Role::Tab, "Editor");
    act(&wb, &snap, editor, Action::Click, None, cx);
    let (snap, tree) = nodes(&wb, cx);
    assert_eq!(find(&tree, Role::Tab, "Editor").1.is_selected(), Some(true));
    act(&wb, &snap, SETTINGS_SEARCH, Action::SetValue, Some(ActionData::Value("no such setting at all".into())), cx);
    let (snap, tree) = nodes(&wb, cx);
    assert_eq!(find(&tree, Role::SearchInput, "Search settings").1.value(), Some("no such setting at all"));
    assert!(tree.iter().any(|(_, n)| n.role() == Role::Status && n.label() == Some("No setting matches.")));

    // A closed page takes its actions with it.
    let (old, tree) = nodes(&wb, cx);
    let _ = snap;
    wb.update(cx, |wb, cx| wb.close_page_for_test(cx));
    act(&wb, &old, find(&tree, Role::Tab, "Commonly Used").0, Action::Click, None, cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), None);
}

#[gpui::test]
fn the_shortcuts_page_lists_commands_and_runs_only_a_listed_one(cx: &mut gpui::TestAppContext) {
    let dir = project("a11y-shortcuts");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("README.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Help: Keyboard Shortcuts Reference", window, cx));
    let (snap, tree) = nodes(&wb, cx);
    let (search, node) = find(&tree, Role::SearchInput, "Search commands or keys (e.g. “comment”, “⌘D”)");
    assert_eq!(node.value(), Some(""));
    assert_eq!(snap.to_tree().focus, search);
    act(&wb, &snap, search, Action::SetValue, Some(ActionData::Value("zen mode".into())), cx);
    let (snap, tree) = nodes(&wb, cx);
    let (row, node) = find(&tree, Role::ListItem, "View: Toggle Zen Mode");
    assert_eq!(node.position_in_set(), Some(1));
    assert!(node.description().is_some(), "the key, or that there is none");
    assert!(tree.iter().any(|(_, n)| n.role() == Role::List && n.label() == Some("1 command")));

    // The list changed: the row is not listed any more, so its old node runs nothing.
    act(&wb, &snap, search, Action::SetValue, Some(ActionData::Value("block comment".into())), cx);
    act(&wb, &snap, row, Action::Click, None, cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("shortcuts", true)), "an outdated row runs nothing");

    act(&wb, &snap, search, Action::SetValue, Some(ActionData::Value("zen mode".into())), cx);
    let (snap, tree) = nodes(&wb, cx);
    act(&wb, &snap, find(&tree, Role::ListItem, "View: Toggle Zen Mode").0, Action::Click, None, cx);
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), None, "the command runs as a click on the row runs it");
}

#[gpui::test]
fn ports_rows_show_their_state_and_act_only_on_the_forward_they_showed(cx: &mut gpui::TestAppContext) {
    use crate::port_forwards::{Effect, ForwardRequest};
    use host::forward::{FailKind, ForwardError, LocalBind, Opened, RemoteTarget};

    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| {
        wb.chrome.panel_open = true;
        wb.chrome.bottom = Bottom::Ports;
        cx.notify();
    });
    let (_, tree) = nodes(&wb, cx);
    assert!(tree.iter().any(|(_, n)| n.role() == Role::Status && n.label() == Some("Port forwarding works while connected to an SSH host.")));

    // Opens a forward of server port `port` that succeeds on `local`, or fails.
    let forward = |wb: &mut Workbench, port: u16, local: Option<u16>| {
        let (id, effects) = wb.ports.set.request(ForwardRequest::user("fixture", port, RemoteTarget::Localhost, LocalBind::Loopback));
        let Some(Effect::Open { seq, .. }) = effects.last() else { panic!("an open") };
        let result = match local {
            Some(local) => Ok(Opened { local, spec: format!("{local}:localhost:{port}") }),
            None => Err(ForwardError { kind: FailKind::AddrInUse, message: "in use".into() }),
        };
        wb.ports.set.on_result(id, *seq, result);
    };
    wb.update(cx, |wb, cx| {
        wb.remote.conn = crate::workbench::Conn::Connected { alias: "fixture".into() };
        forward(wb, 8000, Some(8001));
        forward(wb, 3000, None);
        cx.notify();
    });
    let (snap, tree) = nodes(&wb, cx);
    assert_eq!(find(&tree, Role::TextInput, "Forward a port:").1.value(), Some(""));
    let (row, node) = find(&tree, Role::ListItem, "Port 8000");
    assert_eq!(node.value(), Some("localhost:8001"));
    assert_eq!(node.description(), Some("User Forwarded"));
    let button = |tree: &[(NodeId, Node)], row: NodeId, label: &str| {
        let row = &tree.iter().find(|(id, _)| *id == row).unwrap().1;
        *row.children().iter().find(|id| tree.iter().any(|(i, n)| i == *id && n.label() == Some(label))).unwrap_or_else(|| panic!("{label}"))
    };
    let open_8000 = button(&tree, row, "Open in Browser");
    let stop_8000 = button(&tree, row, "Stop Forwarding");
    let (failed, node) = find(&tree, Role::ListItem, "Port 3000");
    assert!(node.value().is_some_and(|v| !v.is_empty()), "the failure is read");
    let stop_3000 = button(&tree, failed, "Stop Forwarding");

    // Port 8000 is forwarded again meanwhile (another forward, another local port): the old buttons do nothing.
    wb.update(cx, |wb, cx| {
        let id = wb.ports.set.by_remote("fixture", 8000).unwrap().id;
        let _ = wb.ports.set.stop(id);
        forward(wb, 8000, Some(9001));
        cx.notify();
    });
    act(&wb, &snap, open_8000, Action::Click, None, cx);
    assert_eq!(cx.opened_url(), None, "an outdated row opens nothing");
    act(&wb, &snap, stop_8000, Action::Click, None, cx);
    assert!(wb.read_with(cx, |wb, _| wb.ports.set.by_remote("fixture", 8000).is_some()), "an outdated row stops nothing");

    // The rows of the same forward still act, through the pointer's path.
    act(&wb, &snap, stop_3000, Action::Click, None, cx);
    assert!(wb.read_with(cx, |wb, _| wb.ports.set.by_remote("fixture", 3000).is_none()));
    assert!(wb.read_with(cx, |wb, _| wb.ports.ignored.contains(&3000)), "not forwarded automatically again, as after a click");
    let (snap, tree) = nodes(&wb, cx);
    let (row, _) = find(&tree, Role::ListItem, "Port 8000");
    act(&wb, &snap, button(&tree, row, "Open in Browser"), Action::Click, None, cx);
    assert_eq!(cx.opened_url().as_deref(), Some("http://localhost:9001"));
}

/// Rows of the Settings and Keyboard Shortcuts pages report their painted, clipped bounds; rows scrolled out of view
/// have none.
#[gpui::test]
fn page_rows_have_their_painted_bounds_and_hidden_rows_none(cx: &mut gpui::TestAppContext) {
    let dir = project("a11y-page-row-bounds");
    let (wb, cx) = setup(cx);
    cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(420.)));
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Open Settings", window, cx));
    cx.run_until_parked();
    let (_, tree) = nodes(&wb, cx);
    let height = cx.update(|window, _| f64::from(window.viewport_size().height / gpui::px(1.)) * f64::from(window.scale_factor()));
    let (_, theme) = find(&tree, Role::Button, "Theme");
    let rect = theme.bounds().expect("the first setting is on screen");
    assert!(rect.width() > 0. && rect.height() > 0. && rect.y1 <= height + 0.5, "{rect:?}");
    let rows: Vec<&Node> =
        tree.iter().filter(|(id, _)| id.0 >= SET_ROW && id.0 < SHORTCUT_ROW && (id.0 - SET_ROW).is_multiple_of(ROW_IDS)).map(|(_, n)| n).collect();
    assert!(rows.len() > 3, "{} rows", rows.len());
    assert!(rows.iter().any(|n| n.bounds().is_none()), "a row below the fold has no bounds");
    for b in rows.iter().filter_map(|n| n.bounds()) {
        assert!(b.y1 <= height + 0.5, "painted rows lie inside the window: {b:?}");
    }

    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Help: Keyboard Shortcuts Reference", window, cx));
    cx.run_until_parked();
    let (_, tree) = nodes(&wb, cx);
    let shortcuts: Vec<&Node> = tree.iter().filter(|(id, _)| id.0 >= SHORTCUT_ROW && id.0 < SHORTCUT_ROW + 0x1000).map(|(_, n)| n).collect();
    assert!(shortcuts.len() > 20, "{} commands", shortcuts.len());
    assert!(shortcuts[0].bounds().is_some_and(|b| b.height() > 0.), "the first command is on screen");
    assert!(shortcuts.iter().any(|n| n.bounds().is_none()), "a command below the fold has no bounds");
}
