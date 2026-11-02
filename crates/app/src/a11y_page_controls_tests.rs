//! The pages' drawn text and controls in the tree (`a11y_controls.rs`): the merge editor, folder comparison, diff,
//! large-file view, Markdown preview, Welcome page and the containers' overview. Each has its frame, and a press does
//! what a click does.

use accesskit::{Action, ActionRequest, Node, NodeId, Role, TreeId};
use accesskit_consumer::{FilterResult, Tree};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_integration_tests::{wait_secs, wait_ticking};
use crate::qa_workbench_tests::{fresh, open, setup};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn nodes(snap: &Snapshot) -> Vec<(NodeId, Node)> {
    snap.to_tree().nodes
}

fn find(snap: &Snapshot, role: Role, pred: impl Fn(&str) -> bool) -> Option<(NodeId, Node)> {
    nodes(snap).into_iter().find(|(_, n)| n.role() == role && n.label().is_some_and(&pred))
}

fn get(snap: &Snapshot, role: Role, label: &str) -> (NodeId, Node) {
    find(snap, role, |l| l == label).unwrap_or_else(|| panic!("{role:?} {label:?} is in the tree"))
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
    let found = tree.state().root().node_at_point(at, &|_| FilterResult::Include).map(|n| (n.locate().0, n.label()));
    assert_eq!(found.as_ref().map(|f| f.0), Some(id), "{found:?} at {b:?}");
}

const CONFLICTS: &str = "fn main() {\n<<<<<<< HEAD\n    ours();\n=======\n    theirs();\n>>>>>>> feature\n    run();\n<<<<<<< HEAD\n    a();\n=======\n    b();\n>>>>>>> feature\n}\n";

#[gpui::test]
fn the_merge_editor_is_read_and_resolved_from_the_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("a11y-page-merge");
    let file = dir.join("main.rs");
    std::fs::write(&file, CONFLICTS).unwrap();
    let (wb, cx) = setup(cx);
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Merge Conflict: Open Merge Editor", window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    get(&snap, Role::Heading, "Merge main.rs");
    let (_, current) = find(&snap, Role::Document, |l| l.starts_with("Current")).expect("the current side");
    assert_eq!(current.value(), Some("    ours();"));
    assert!(find(&snap, Role::Button, |l| l == "Accept All Incoming").is_some());
    let accept = nodes(&snap)
        .into_iter()
        .find(|(_, n)| n.role() == Role::Button && n.label() == Some("Accept Current") && n.description() == Some("Conflict 1 of 2"))
        .expect("the first conflict's Accept Current");
    hit(&snap, accept.0);
    press(&wb, cx, &snap, accept.0);
    assert!(ed.read_with(cx, |e, _| e.text()).starts_with("fn main() {\n    ours();\n    run();\n<<<<<<< HEAD"));
}

#[gpui::test]
fn the_diff_and_the_large_file_are_read_line_by_line(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = crate::qa_integration_tests::repo("a11y-page-diff");
    std::fs::write(dir.join("src/lib.rs"), (1..=10).map(|i| if i == 3 { "// new\n".to_string() } else { format!("// line {i}\n") }).collect::<String>())
        .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| crate::qa_integration_tests::changes(&wb, cx).is_some_and(|c| !c.is_empty()));
    wb.update_in(cx, |wb, window, cx| wb.open_diff("src/lib.rs".into(), false, window, cx));
    wait_ticking(cx, "diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let (added, _) = get(&snap, Role::ListItem, "Added, line 3: // new");
    hit(&snap, added);
    assert!(find(&snap, Role::ListItem, |l| l.starts_with("Removed, line 3:")).is_some());
    let (toggle, _) = get(&snap, Role::Button, "Toggle inline view");
    press(&wb, cx, &snap, toggle);
    let view = wb.read_with(cx, |wb, _| wb.diff_view()).unwrap();
    assert!(view.read_with(cx, |v, _| v.is_inline()), "inline now");

    // A large file: each line drawn is an item; Follow is a check box.
    let big = dir.join("big.log");
    std::fs::write(&big, "first line\nsecond line\n").unwrap();
    let path = big.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_large_for_test(&path, window, cx));
    wait_secs(cx, 10, "the lines", |cx| find(&snapshot(&wb, cx), Role::ListItem, |l| l == "line 2: second line").is_some());
    let snap = snapshot(&wb, cx);
    let (_, follow) = get(&snap, Role::CheckBox, "Follow");
    assert!(follow.toggled().is_some());
}

#[gpui::test]
fn markdown_preview_and_welcome(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("a11y-page-markdown");
    std::fs::write(dir.join("README.md"), "# Title\n\nSee the [docs](https://example.com/docs) first.\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("README.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let (title, _) = get(&snap, Role::Heading, "Title");
    hit(&snap, title);
    get(&snap, Role::Paragraph, "See the docs first.");
    let (_, link) = get(&snap, Role::Link, "docs");
    assert_eq!(link.description(), Some("https://example.com/docs"));
    assert!(link.supports_action(Action::Click));

    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Help: Welcome", window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let group = nodes(&snap).into_iter().find(|(_, n)| n.role() == Role::Group && n.label() == Some("Welcome")).expect("the Welcome page's group");
    let buttons: Vec<NodeId> = group.1.children().to_vec();
    assert!(buttons.len() >= 3, "its tiles");
    for id in buttons {
        hit(&snap, id);
    }
}

#[gpui::test]
fn the_folder_comparison_from_the_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (l, r) = (fresh("a11y-page-compare-l"), fresh("a11y-page-compare-r"));
    std::fs::write(l.join("a.txt"), "one\n").unwrap();
    std::fs::write(r.join("a.txt"), "two\n").unwrap();
    std::fs::write(l.join("only.txt"), "x\n").unwrap();
    let (wb, cx) = setup(cx);
    let (ls, rs) = (l.to_string_lossy().into_owned(), r.to_string_lossy().into_owned());
    wb.update_in(cx, |wb, window, cx| wb.open_folder_compare_for_test((true, &ls), (true, &rs), window, cx));
    let view = wb.read_with(cx, |wb, _| wb.compare_view_for_test()).expect("the compare tab is open");
    wait_secs(cx, 20, "the comparison finishes", |cx| !view.read_with(cx, |v, _| v.scanning()));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let (row, item) = get(&snap, Role::TreeItem, "only.txt");
    assert!(item.description().is_some());
    hit(&snap, row);
    let (ws, n) = get(&snap, Role::CheckBox, "Ignore whitespace");
    let was = n.toggled();
    press(&wb, cx, &snap, ws);
    assert_ne!(get(&snapshot(&wb, cx), Role::CheckBox, "Ignore whitespace").1.toggled(), was, "toggled as a click does");
    assert!(find(&snapshot(&wb, cx), Role::Button, |l| l == "Copy to the right").is_some());
}

#[gpui::test]
fn the_containers_overview_from_the_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (wb, cx) = setup(cx);
    let source: std::sync::Arc<dyn host::Host> = crate::container_monitor::tests::FixtureHost::new();
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().docker_enabled = true);
    wb.update(cx, |wb, cx| {
        wb.remote.host = source.clone();
        wb.remote.conn = Conn::Connected { alias: "fixture".into() };
        let m = &mut wb.remote.docker.monitor;
        m.host = Some("fixture".into());
        m.source = Some(source.clone());
        m.apply(Ok(crate::container_monitor::read(source.as_ref(), "fixture", true, &std::sync::atomic::AtomicBool::new(false)).unwrap()));
        m.paused = true;
        wb.open_container_dashboard(cx);
    });
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let group = nodes(&snap).into_iter().find(|(_, n)| n.role() == Role::Group && n.label() == Some("Containers")).expect("the page's controls");
    let rows: Vec<(NodeId, Node)> = nodes(&snap).into_iter().filter(|(id, n)| group.1.children().contains(id) && n.role() == Role::ListItem).collect();
    assert!(!rows.is_empty(), "the containers are listed");
    assert!(find(&snap, Role::SearchInput, |l| l == "Search").is_some());
    assert!(find(&snap, Role::Button, |l| l == "Needs attention").is_some());
    hit(&snap, rows[0].0);
    press(&wb, cx, &snap, rows[0].0);
    wait_ticking(cx, "the selection", |cx| wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.selected.is_some()));
    assert!(wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.overview.details), "its details are shown");
}
