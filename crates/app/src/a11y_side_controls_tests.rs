//! The side views' drawn controls in the tree (`a11y_controls.rs`): the Explorer's header buttons, sections, open
//! editors and outline; the Search view's buttons; Run and Debug's Remove All Breakpoints and launch link; the Testing
//! filter; the Extensions header and an installed extension's page. Each has its frame, and a press does what a click
//! does.

use accesskit::{Action, ActionRequest, Node, NodeId, Role, TreeId};
use accesskit_consumer::{FilterResult, Tree};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_workbench_tests::{open, project, setup};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn find(snap: &Snapshot, role: Role, label: &str) -> Option<(NodeId, Node)> {
    snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == role && n.label() == Some(label))
}

fn get(snap: &Snapshot, role: Role, label: &str) -> (NodeId, Node) {
    find(snap, role, label).unwrap_or_else(|| panic!("{role:?} {label:?} is in the tree"))
}

fn press(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId, action: Action) {
    let req = ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

/// AccessKit's hit test at the centre of `id`'s frame finds it.
fn hit(snap: &Snapshot, id: NodeId) {
    let update = snap.to_tree();
    let b = update.nodes.iter().find(|(i, _)| *i == id).and_then(|(_, n)| n.bounds()).unwrap_or_else(|| panic!("{id:?} has a frame"));
    let tree = Tree::new(update, true);
    let at = accesskit::Point::new((b.x0 + b.x1) / 2., (b.y0 + b.y1) / 2.);
    let found = tree.state().root().node_at_point(at, &|_| FilterResult::Include).map(|n| (n.locate().0, n.label(), n.role(), n.data().bounds()));
    assert_eq!(found.as_ref().map(|f| f.0), Some(id), "{found:?} at {b:?}");
}

fn show(wb: &Entity<Workbench>, cx: &mut VisualTestContext, panel: Panel) {
    wb.update(cx, |wb, cx| {
        wb.chrome.panel = Some(panel);
        cx.notify();
    });
    cx.run_until_parked();
}

#[gpui::test]
fn the_explorer_header_sections_and_open_editors(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = project("a11y-side-explorer");
    let (wb, cx) = setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().open_editors = true);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("README.md"), cx);
    // An edit keeps it open (an untouched file opened in preview is replaced by the next one).
    cx.simulate_keystrokes("x");
    open(&wb, &dir.join("src/main.rs"), cx);
    let snap = snapshot(&wb, cx);
    for name in ["New File… (⌘N)", "New Folder…", "Refresh Explorer", "Collapse Folders"] {
        let (id, _) = get(&snap, Role::Button, name);
        hit(&snap, id);
    }
    // A section's header: a button with its state; a press folds it as a click does.
    let (outline, node) = snap
        .to_tree()
        .nodes
        .into_iter()
        .find(|(_, n)| n.role() == Role::Button && n.label().is_some_and(|l| l.starts_with("Outline")))
        .expect("the Outline header");
    let was = node.is_expanded().expect("it says whether it is open");
    press(&wb, cx, &snap, outline, Action::Click);
    let now = snapshot(&wb, cx).to_tree().nodes.into_iter().find(|(id, _)| *id == outline).and_then(|(_, n)| n.is_expanded());
    assert_eq!(now, Some(!was), "folded or unfolded");

    // Open Editors (opened if it is not): the file is a list item; its Close closes it.
    let snap = snapshot(&wb, cx);
    if let Some((head, n)) =
        snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Button && n.label().is_some_and(|l| l.starts_with("Open Editors")))
        && n.is_expanded() == Some(false)
    {
        press(&wb, cx, &snap, head, Action::Click);
    }
    let snap = snapshot(&wb, cx);
    let (row, item) = get(&snap, Role::ListItem, "main.rs");
    assert_eq!(item.is_selected(), Some(true));
    hit(&snap, row);
    let close = snap
        .to_tree()
        .nodes
        .into_iter()
        .find(|(_, n)| n.role() == Role::Button && n.label() == Some("Close") && n.description() == Some("main.rs"))
        .expect("its Close");
    press(&wb, cx, &snap, close.0, Action::Click);
    assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.editors.len()), 1, "main.rs closed, the edited README stays");
}

#[gpui::test]
fn the_search_views_buttons(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = project("a11y-side-search");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Search: Find in Files", window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let (query, _) = get(&snap, Role::SearchInput, "Search");
    let req =
        ActionRequest { action: Action::SetValue, target_tree: TreeId::ROOT, target_node: query, data: Some(accesskit::ActionData::Value("line".into())) };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, &snap, window, cx));
    crate::qa_integration_tests::wait_ticking(cx, "the search", |cx| wb.read_with(cx, |wb, cx| wb.search_summary_for_test(cx)).is_some());
    let snap = snapshot(&wb, cx);
    for name in ["Next result (F4)", "Previous result (⇧F4)", "Open in a Search Editor tab", "Run the search again"] {
        let (id, _) = get(&snap, Role::Button, name);
        hit(&snap, id);
    }
    assert!(find(&snap, Role::Button, "Collapse all results").or(find(&snap, Role::Button, "Expand all results")).is_some());
    let (clear, _) = get(&snap, Role::Button, "Clear the search");
    press(&wb, cx, &snap, clear, Action::Click);
    assert!(wb.read_with(cx, |wb, cx| wb.search_summary_for_test(cx)).is_none(), "the results are gone");
    assert!(find(&snapshot(&wb, cx), Role::Button, "Next result (F4)").is_none(), "with their buttons");
}

#[gpui::test]
fn run_and_debug_and_testing(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = project("a11y-side-debug");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("src/lib.rs"), cx);
    cx.simulate_keystrokes("down");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Debug: Toggle Breakpoint", window, cx));
    show(&wb, cx, Panel::Debug);
    let snap = snapshot(&wb, cx);
    assert!(find(&snap, Role::Button, "Go to Breakpoint").is_some());
    let (link, _) = get(&snap, Role::Link, "Create a launch configuration");
    hit(&snap, link);
    let (clear, _) = get(&snap, Role::Button, "Remove All Breakpoints");
    hit(&snap, clear);
    press(&wb, cx, &snap, clear, Action::Click);
    assert!(find(&snapshot(&wb, cx), Role::Button, "Go to Breakpoint").is_none(), "no breakpoint is left");

    // Testing: the filter field takes the keyboard from the tree.
    let snap = snapshot(&wb, cx);
    let (testing, _) = get(&snap, Role::Tab, "Testing");
    press(&wb, cx, &snap, testing, Action::Click);
    let snap = snapshot(&wb, cx);
    let (filter, node) = get(&snap, Role::TextInput, "Filter (e.g. text, path)");
    assert!(node.supports_action(Action::Focus));
    hit(&snap, filter);
    press(&wb, cx, &snap, filter, Action::Focus);
    assert_eq!(snapshot(&wb, cx).to_tree().focus, filter, "the field has the keyboard");
    assert!(find(&snapshot(&wb, cx), Role::Button, "Collapse All").is_some());
}

#[gpui::test]
fn the_extensions_header_and_an_installed_page(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    if !crate::ext_inspector_ui_tests::exthost_built() {
        return;
    }
    let cfg = crate::ext_inspector_ui_tests::config_dir("a11y-side-page");
    crate::ext_inspector_ui_tests::install(&cfg, "1.0.0");
    let (wb, cx) = crate::ext_inspector_ui_tests::setup(cx, &cfg);
    let snap = snapshot(&wb, cx);
    let (view, _) = get(&snap, Role::Tab, "Extensions");
    press(&wb, cx, &snap, view, Action::Click);
    let snap = snapshot(&wb, cx);
    for name in ["Install from File…", "Install from Folder…"] {
        let (id, _) = get(&snap, Role::Button, name);
        hit(&snap, id);
    }
    wb.update(cx, |wb, cx| wb.select_extension_for_test("you.probe", cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let page = snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Group && n.label() == Some("Probe")).expect("the page's group");
    let roles: Vec<Role> = snap.to_tree().nodes.iter().filter(|(id, _)| page.1.children().contains(id)).map(|(_, n)| n.role()).collect();
    assert!(roles.contains(&Role::Heading) && roles.contains(&Role::Switch), "{roles:?}");
    let (net, switch) = snap
        .to_tree()
        .nodes
        .into_iter()
        .find(|(_, n)| n.role() == Role::Switch && n.label().is_some_and(|l| l.contains("api.example.com")))
        .expect("the network switch");
    assert_eq!(switch.toggled(), Some(accesskit::Toggled::True));
    press(&wb, cx, &snap, net, Action::Click);
    let revoked = crate::ext::store(&cfg).revoked_permissions(&ext_manifest::ExtId::parse("you.probe").unwrap(), &ext_core::Source::File { path: "x".into() });
    assert_eq!(revoked.net, ["api.example.com"], "turned off as a click does");
    let (disable, _) = get(&snapshot(&wb, cx), Role::Button, "Disable");
    let snap = snapshot(&wb, cx);
    hit(&snap, disable);
    press(&wb, cx, &snap, disable, Action::Click);
    assert!(find(&snapshot(&wb, cx), Role::Button, "Enable").is_some(), "disabled");
}

/// Source Control: Refresh and More Actions in the header; the Stashes header folds; a stash's Apply applies it.
#[gpui::test]
fn source_control_header_and_stashes(cx: &mut TestAppContext) {
    use crate::qa_integration_tests::{git_in, wait_ticking};
    let _en = crate::i18n::LangGuard::set("en");
    let Some(dir) = crate::tests::git_repo("a11y-side-scm") else { return };
    std::fs::write(dir.join("tracked.txt"), "before\n").unwrap();
    git_in(&dir, &["add", "tracked.txt"]);
    git_in(&dir, &["commit", "-qm", "Add tracked fixture"]);
    std::fs::write(dir.join("tracked.txt"), "after\n").unwrap();
    git_in(&dir, &["stash", "push", "-qm", "kept work"]);
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    let stashes =
        |snap: &Snapshot| snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Button && n.label().is_some_and(|l| l.starts_with("Stashes (1)")));
    wait_ticking(cx, "the stash", |cx| stashes(&snapshot(&wb, cx)).is_some());
    let snap = snapshot(&wb, cx);
    for name in ["Refresh", "More Actions…"] {
        assert!(find(&snap, Role::Button, name).is_some(), "{name}");
    }
    let (head, node) = stashes(&snap).unwrap();
    hit(&snap, head);
    if node.is_expanded() == Some(false) {
        press(&wb, cx, &snap, head, Action::Click);
    }
    let snap = snapshot(&wb, cx);
    assert_eq!(stashes(&snap).unwrap().1.is_expanded(), Some(true));
    let apply = snap
        .to_tree()
        .nodes
        .into_iter()
        .find(|(_, n)| n.role() == Role::Button && n.label() == Some("Apply (keep the stash)") && n.description().is_some_and(|d| d.ends_with("kept work")))
        .expect("the stash's Apply");
    for name in ["Pop (apply and drop)", "Drop"] {
        assert!(find(&snap, Role::Button, name).is_some(), "{name}");
    }
    press(&wb, cx, &snap, apply.0, Action::Click);
    wait_ticking(cx, "the stash applied", |_| std::fs::read_to_string(dir.join("tracked.txt")).is_ok_and(|t| t == "after\n"));
}
