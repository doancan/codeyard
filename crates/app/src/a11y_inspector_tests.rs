use super::*;
use crate::ext_inspector_ui_tests::{config_dir, exthost_built, install, setup};
use gpui::{TestAppContext, VisualTestContext};

/// The first node under `node` (itself included) that `pred` accepts.
fn find<'a>(node: accesskit_consumer::NodeRef<'a>, pred: &dyn Fn(&accesskit_consumer::NodeRef<'a>) -> bool) -> Option<accesskit_consumer::NodeRef<'a>> {
    if pred(&node) {
        return Some(node);
    }
    node.children().find_map(|c| find(c, pred))
}

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

/// AC4: the Inspector opens from the palette with the keyboard on it; a screen reader reads a heading per extension and
/// section and a list item per line, and **Copy fix** after a denial is a button that copies the manifest line.
#[gpui::test]
fn the_inspector_is_read_and_driven_by_a_screen_reader(cx: &mut TestAppContext) {
    if !exthost_built() {
        return;
    }
    let cfg = config_dir("a11y");
    install(&cfg, "1.0.0");
    let (wb, cx) = setup(cx, &cfg);
    wb.update(cx, |wb, _| wb.record_denial_for_test("you.probe", "workspace/read-file", "read = [\"src/a.rs\"]", ext_api::value::Why::NotDeclared));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Developer: Extension Inspector", window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("extension-inspector", true)));
    assert!(cx.debug_bounds("insp-line-1").is_some(), "rows are drawn");

    let snap = snapshot(&wb, cx);
    let inspector = snap.inspector.as_ref().expect("the Inspector is in front");
    let ix = inspector.lines.iter().position(|l| l.fix.is_some()).expect("the denial has a fix");
    let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
    let state = tree.state();
    let group = find(state.root(), &|n| n.role() == Role::Group && n.label().as_deref() == Some("Extension Inspector")).expect("the group");
    let headings: Vec<String> = group.children().filter(|n| n.role() == Role::Heading).filter_map(|n| n.label()).collect();
    assert!(headings.iter().any(|h| h.starts_with("you.probe 1.0.0")), "{headings:?}");
    assert!(headings.iter().any(|h| h == "Denials"), "{headings:?}");
    let row = find(group, &|n| n.role() == Role::ListItem && n.label().is_some_and(|l| l.contains("workspace/read-file"))).expect("the denial");
    assert!(row.label().unwrap().contains("not declared"), "{:?}", row.label());
    let button = find(group, &|n| n.role() == Role::Button).expect("Copy fix");
    assert_eq!(button.label().as_deref(), Some("Copy fix"));
    assert_eq!(button.description().as_deref(), Some("read = [\"src/a.rs\"]"));

    let request = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: NodeId(line_id(ix).0 + 1), data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&request, &snap, window, cx));
    cx.run_until_parked();
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some("read = [\"src/a.rs\"]"));
}
