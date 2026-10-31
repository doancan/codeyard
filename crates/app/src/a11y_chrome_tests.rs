//! The window's chrome and the controls views declare where they draw them (`a11y_controls.rs`): the activity bar,
//! the status bar's items, the editor tabs' large-file, diff and page tabs and their buttons, another group's tabs,
//! the panel's buttons and terminals, breadcrumbs, the resource monitor, the error card, the Run and Debug selector and
//! a breakpoint's place. Each has its frame, and a press does what a click does.

use accesskit::{Action, ActionRequest, Node, NodeId, Role, TreeId};
use accesskit_consumer::{FilterResult, Tree};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_workbench_tests::{fresh, open, setup};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn find(snap: &Snapshot, role: Role, label: &str) -> Option<(NodeId, Node)> {
    snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == role && n.label() == Some(label))
}

fn get(snap: &Snapshot, role: Role, label: &str) -> (NodeId, Node) {
    find(snap, role, label).unwrap_or_else(|| panic!("{role:?} {label:?} is in the tree"))
}

/// A button named `label` whose description is `of` (a tab's Close, a breakpoint's Go to Breakpoint).
fn button_of(snap: &Snapshot, label: &str, of: &str) -> NodeId {
    snap.to_tree()
        .nodes
        .into_iter()
        .find(|(_, n)| n.role() == Role::Button && n.label() == Some(label) && n.description() == Some(of))
        .unwrap_or_else(|| panic!("{label} of {of}"))
        .0
}

fn press(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId) {
    let req = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

/// AccessKit's hit test at the centre of `id`'s frame finds it.
fn hit(snap: &Snapshot, id: NodeId) {
    let update = snap.to_tree();
    let b = update.nodes.iter().find(|(i, _)| *i == id).and_then(|(_, n)| n.bounds()).unwrap_or_else(|| panic!("{id:?} has a frame"));
    let tree = Tree::new(update, true);
    let at = accesskit::Point::new((b.x0 + b.x1) / 2., (b.y0 + b.y1) / 2.);
    assert_eq!(tree.state().root().node_at_point(at, &|_| FilterResult::Include).map(|n| n.locate().0), Some(id));
}

#[gpui::test]
fn the_activity_bar_and_status_bar_act_like_their_clicks(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("a11y-chrome-bars");
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("a.txt"), cx);
    let snap = snapshot(&wb, cx);
    let (explorer, tab) = get(&snap, Role::Tab, "Explorer (⇧⌘E)");
    assert_eq!(tab.is_selected(), Some(true));
    hit(&snap, explorer);
    let (search, _) = get(&snap, Role::Tab, "Search (⇧⌘F)");
    press(&wb, cx, &snap, search);
    assert!(wb.read_with(cx, |wb, _| wb.view_shown(Panel::Search)), "the Search view is shown");
    // The same press from the old tree (Search was hidden then) does not hide it again.
    press(&wb, cx, &snap, search);
    assert!(wb.read_with(cx, |wb, _| wb.view_shown(Panel::Search)));

    // Status bar: the items are buttons; the problems item opens the panel's Problems, the terminal button the panel.
    let snap = snapshot(&wb, cx);
    let (problems, _) = get(&snap, Role::Button, "0 errors, 0 warnings");
    hit(&snap, problems);
    press(&wb, cx, &snap, problems);
    assert!(wb.read_with(cx, |wb, _| wb.chrome.panel_open && wb.chrome.bottom == Bottom::Problems));
    let snap = snapshot(&wb, cx);
    let (toggle, _) = get(&snap, Role::Button, "Toggle Panel (⌘J)");
    hit(&snap, toggle);
    press(&wb, cx, &snap, toggle);
    assert!(!wb.read_with(cx, |wb, _| wb.chrome.panel_open), "the panel is hidden");
    let (resources, node) = get(&snapshot(&wb, cx), Role::Button, "Resources");
    assert_eq!(node.is_expanded(), Some(false));
    let snap = snapshot(&wb, cx);
    press(&wb, cx, &snap, resources);
    assert_eq!(get(&snapshot(&wb, cx), Role::Button, "Resources").1.is_expanded(), Some(true));
    // The figures slide open (200 ms): the tree lists them once they are laid out.
    let cpu = |snap: &Snapshot| snap.to_tree().nodes.iter().any(|(_, n)| n.role() == Role::Label && n.label().is_some_and(|l| l.starts_with("CPU: ")));
    crate::qa_integration_tests::wait_ticking(cx, "the figures", |cx| cpu(&snapshot(&wb, cx)));
}

/// Page, diff and large-file tabs are tabs of the list; their Close buttons close them; a breadcrumb shows the file
/// in the Explorer; another group's tabs are listed under that group and bring it forward.
#[gpui::test]
fn tabs_breadcrumbs_and_other_groups(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("a11y-chrome-tabs");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.txt"), "a\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("src/a.txt"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Open Settings", window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let list = snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::TabList && n.label() == Some("Editor tabs")).unwrap().1;
    let (settings, tab) = get(&snap, Role::Tab, "Settings");
    assert!(list.children().contains(&settings), "the page tab is in the list");
    assert_eq!(tab.is_selected(), Some(true));
    hit(&snap, settings);
    // Back to the editor, then the page from the tree.
    wb.update_in(cx, |wb, window, cx| wb.activate(0, window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    press(&wb, cx, &snap, settings);
    assert!(wb.read_with(cx, |wb, _| wb.tabs.page_active), "the page is in front again");
    let snap = snapshot(&wb, cx);
    let close = button_of(&snap, "Close", "Settings");
    press(&wb, cx, &snap, close);
    assert!(wb.read_with(cx, |wb, _| wb.tabs.page.is_none()), "closed");

    // A breadcrumb: the folder is shown in the Explorer.
    wb.update(cx, |wb, cx| {
        wb.chrome.panel = Some(Panel::Search);
        cx.notify();
    });
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let (crumb, _) = get(&snap, Role::Link, "a.txt");
    press(&wb, cx, &snap, crumb);
    assert!(wb.read_with(cx, |wb, _| wb.view_shown(Panel::Explorer)));

    // Another group: its tab is listed as "Editor group 1" once the second group has the focus.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Split Editor", window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let group = snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Group && n.label() == Some("Editor group 1")).expect("the first group");
    let tab = group.1.children()[0];
    press(&wb, cx, &snap, tab);
    let focused = wb.read_with(cx, |wb, _| wb.tabs.layout.focused());
    let first = wb.read_with(cx, |wb, _| wb.tabs.layout.groups()[0]);
    assert_eq!(focused, first, "the first group has the focus");
}

/// The panel's buttons: New Terminal adds one, which is listed under "Terminals" with its Kill button.
#[gpui::test]
fn panel_buttons_and_the_terminal_list(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let (new, _) = get(&snap, Role::Button, "New Terminal (⌃⇧`)");
    hit(&snap, new);
    press(&wb, cx, &snap, new);
    assert_eq!(wb.read_with(cx, |wb, _| wb.terminals.len()), 2);
    let snap = snapshot(&wb, cx);
    let terms = snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Group && n.label() == Some("Terminals")).expect("the terminal list");
    assert!(terms.1.children().len() >= 4, "two terminals, each with Kill");
    for name in ["Split Terminal", "Kill Terminal", "Maximize Panel", "Hide Panel (⌘J)"] {
        assert!(find(&snap, Role::Button, name).is_some(), "{name}");
    }
}

/// The error card's Close button clears it.
#[gpui::test]
fn the_error_card_closes_from_the_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| {
        wb.notice = Some(Notice::Error("Something failed".into()));
        cx.notify();
    });
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let group = snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Group && n.label() == Some("Error")).expect("the card's buttons");
    press(&wb, cx, &snap, group.1.children()[0]);
    assert!(wb.read_with(cx, |wb, _| wb.notice.is_none()));
}

/// Run and Debug: the selector opens its menu; a line breakpoint has Go to Breakpoint, which opens its place.
#[gpui::test]
fn the_configuration_selector_and_a_breakpoints_place(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("a11y-chrome-debug");
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("a.txt"), cx);
    cx.simulate_keystrokes("down");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Debug: Toggle Breakpoint", window, cx));
    cx.simulate_keystrokes("cmd-down");
    wb.update(cx, |wb, cx| {
        wb.chrome.panel = Some(Panel::Debug);
        cx.notify();
    });
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let go = button_of(&snap, "Go to Breakpoint", "a.txt, line 2");
    press(&wb, cx, &snap, go);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position().0), 2, "the breakpoint's line");
    let snap = snapshot(&wb, cx);
    let chooser =
        snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Button && n.description() == Some("Launch configuration")).expect("the selector");
    press(&wb, cx, &snap, chooser.0);
    assert!(wb.read_with(cx, |wb, _| wb.menu.is_some()), "the menu opened");
    assert!(snapshot(&wb, cx).to_tree().nodes.iter().any(|(_, n)| n.role() == Role::Menu), "its menu is open");
}
