use super::*;
use gpui::{TestAppContext, VisualTestContext};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn click(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId) {
    let request = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&request, snap, window, cx));
    cx.run_until_parked();
}

/// AC8: terminals in the editor area are tabs of the tab list; a click brings one to the front like the pointer, and a
/// click from before the tab moved away changes nothing.
#[gpui::test]
fn terminal_tabs_are_in_the_tab_list_and_a_click_selects_them(cx: &mut TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    for _ in 0..2 {
        wb.update_in(cx, |wb, window, cx| {
            wb.new_terminal(&NewTerminal, window, cx);
            wb.move_terminal_into_editor(window, cx);
        });
        cx.run_until_parked();
    }
    let terms = wb.read_with(cx, |wb, _| wb.editor_terminals_for_test());
    assert_eq!(terms.len(), 2);
    let snap = snapshot(&wb, cx);
    assert_eq!(snap.term_tabs.iter().map(|t| t.active).collect::<Vec<_>>(), [false, true], "the last one moved is in front");

    let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
    let state = tree.state();
    let list = state.root().children().find(|n| n.role() == Role::TabList).expect("the tab list");
    let tabs: Vec<_> = list.children().collect();
    assert_eq!(tabs.len(), 2);
    assert!(tabs.iter().all(|t| t.role() == Role::Tab && t.label().is_some_and(|l| l.ends_with("(terminal)"))));
    assert!(tabs[1].is_selected() == Some(true) && tabs[0].is_selected() == Some(false));
    // Each tab is where it is drawn, side by side, and AccessKit's hit test finds it there.
    let frames: Vec<accesskit::Rect> = tabs.iter().map(|t| t.data().bounds().expect("the tab's frame")).collect();
    assert!(frames[0].x1 <= frames[1].x0 + 0.5, "{frames:?}");
    for (tab, f) in tabs.iter().zip(&frames) {
        let at = accesskit::Point::new((f.x0 + f.x1) / 2., (f.y0 + f.y1) / 2.);
        assert_eq!(state.root().node_at_point(at, &|_| accesskit_consumer::FilterResult::Include).map(|n| n.id()), Some(tab.id()));
    }

    click(&wb, cx, &snap, NodeId(TERM_TAB));
    assert_eq!(wb.read_with(cx, |wb, _| wb.front_editor_terminal()), Some(terms[0].clone()), "the click selected it");
    assert!(cx.update(|window, cx| terms[0].read(cx).focus_handle(cx).contains_focused(window, cx)), "with the keyboard in it");

    // The first terminal goes back to the panel; the old snapshot's first tab is stale.
    let old = snapshot(&wb, cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.return_editor_terminal(&terms[0], window, cx);
        wb.show_editor_terminal(terms[1].clone(), window, cx);
    });
    cx.run_until_parked();
    click(&wb, cx, &old, NodeId(TERM_TAB));
    wb.read_with(cx, |wb, _| {
        assert_eq!(wb.front_editor_terminal(), Some(terms[1].clone()), "a stale click changes nothing");
        assert_eq!(wb.editor_terminals_for_test(), vec![terms[1].clone()]);
    });
    let snap = snapshot(&wb, cx);
    assert_eq!(snap.term_tabs.len(), 1);
    assert_eq!(snap.term_tabs[0].entity, terms[1].entity_id());
}
