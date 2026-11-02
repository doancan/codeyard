use super::*;
use crate::market_tests::{dir, open_market, registry, setup};
use gpui::{TestAppContext, VisualTestContext};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

/// The first node under `node` (itself included) that `pred` accepts.
fn find<'a>(node: accesskit_consumer::NodeRef<'a>, pred: &dyn Fn(&accesskit_consumer::NodeRef<'a>) -> bool) -> Option<accesskit_consumer::NodeRef<'a>> {
    if pred(&node) {
        return Some(node);
    }
    node.children().find_map(|c| find(c, pred))
}

fn act(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId, action: Action) {
    let request = ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&request, snap, window, cx));
    cx.run_until_parked();
}

/// AC4: the result count is a live status a screen reader speaks while typing, the search field has the focus, rows
/// are options of the list it controls, and the tabs, rows and Install button act like clicks.
#[gpui::test]
fn the_market_is_announced_and_driven_by_a_screen_reader(cx: &mut TestAppContext) {
    let root = dir("a11y");
    let (_r, _fetch) = registry(&root, "a11y", crate::ext_registry::now());
    let (wb, cx) = setup(cx, &root.join("cfg"));
    let snap = snapshot(&wb, cx);
    assert!(!snap.exts.as_ref().unwrap().market, "the Installed tab first");
    act(&wb, cx, &snap, EXT_TAB_MARKET, Action::Click);
    cx.simulate_input("light");
    cx.run_until_parked();

    let snap = snapshot(&wb, cx);
    let view = snap.exts.as_ref().unwrap();
    assert!(view.market && view.search_focused);
    assert_eq!((view.query.as_str(), view.status.as_str()), ("light", "1 extension"));
    assert_eq!(view.list.rows[0].0, "Dawn");
    assert!(view.list.frames[0].is_some_and(|f| f.width() > 0. && f.height() > 0.), "the Market row has its painted frame");
    let update = snap.to_tree();
    assert_eq!(update.focus, EXT_SEARCH);
    let tree = accesskit_consumer::Tree::new(update, true);
    let state = tree.state();
    // The field keeps the keys; a screen reader is told about the row it highlights (its active descendant).
    let focus = state.focus().unwrap();
    assert_eq!((focus.role(), focus.label().as_deref()), (Role::ListBoxOption, Some("Dawn")));
    let status = find(state.root(), &|n| n.role() == Role::Status && n.label().as_deref() == Some("1 extension")).expect("the live status");
    assert_eq!(status.live(), Live::Polite);
    let option = find(state.root(), &|n| n.role() == Role::ListBoxOption).expect("a result row");
    assert_eq!(option.label().as_deref(), Some("Dawn"));
    assert_eq!(option.is_selected(), Some(true), "the highlighted row");

    // A click on the row opens its page; the page's Install button opens the consent sheet.
    act(&wb, cx, &snap, NodeId(EXT_ROW), Action::Click);
    let snap = snapshot(&wb, cx);
    let page = snap.ext_page.as_ref().expect("the page is in front");
    assert_eq!((page.title.as_str(), page.install.as_deref()), ("Dawn", Some("Install")));
    act(&wb, cx, &snap, EXT_PAGE_INSTALL, Action::Click);
    assert!(wb.read_with(cx, |wb, _| wb.consent_source_for_test()).is_some(), "the consent sheet");

    // A row that is no longer the same extension does nothing (opening a page would drop the sheet).
    open_market(&wb, cx);
    wb.update_in(cx, |wb, _, cx| wb.market_search_for_test("forest", cx));
    cx.run_until_parked();
    let stale = snapshot(&wb, cx);
    wb.update_in(cx, |wb, _, cx| wb.market_search_for_test("night", cx));
    cx.run_until_parked();
    act(&wb, cx, &stale, NodeId(EXT_ROW), Action::Click);
    assert!(wb.read_with(cx, |wb, _| wb.consent_source_for_test()).is_some(), "still the sheet: the stale click was ignored");
}
