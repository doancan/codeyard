//! The Explorer is in the accessibility tree only while a side bar shows it: switching the view, hiding the side bar
//! or zen mode take it out, and the actions of a tree read before (focus, scroll, a row's focus and click) do nothing
//! on the hidden Explorer.

use accesskit::{Action, ActionRequest, Role, TreeId};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_integration_tests::wait_ticking;
use crate::qa_workbench_tests::{fresh, open, setup};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn request(wb: &Entity<Workbench>, cx: &mut VisualTestContext, old: &Snapshot, target_node: NodeId, action: Action) {
    let req = ActionRequest { action, target_tree: TreeId::ROOT, target_node, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, old, window, cx));
    cx.run_until_parked();
}

fn labelled(snap: &Snapshot, label: &str) -> NodeId {
    snap.to_tree().nodes.into_iter().find(|(_, n)| n.label() == Some(label)).map(|(id, _)| id).unwrap_or_else(|| panic!("missing node: {label}"))
}

/// A folder with a subfolder and a file, its Explorer read.
fn fixture<'a>(name: &str, cx: &'a mut TestAppContext) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    let dir = fresh(name);
    std::fs::create_dir(dir.join("folder")).unwrap();
    std::fs::write(dir.join("folder/inner.txt"), "inner\n").unwrap();
    std::fs::write(dir.join("z.txt"), "z\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "the tree", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).len() == 2);
    (wb, cx)
}

/// What an Explorer action would change: the selection, the open file, the focus on a tree, the scroll position.
fn explorer_state(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> (Option<String>, Option<String>, bool, Option<(usize, usize)>) {
    wb.update_in(cx, |wb, window, cx| {
        let tree = wb.all_trees()[0].read(cx);
        (wb.tree_selected(cx), wb.active_path(cx), tree.focus_handle(cx).is_focused(window), tree.visible_rows())
    })
}

/// Every Explorer action of an old tree: focus and scroll the tree, focus and click a row.
fn replay_explorer(wb: &Entity<Workbench>, cx: &mut VisualTestContext, old: &Snapshot) {
    for action in [Action::Focus, Action::ScrollDown, Action::ScrollUp] {
        request(wb, cx, old, EXPLORER, action);
    }
    let row = labelled(old, "z.txt");
    request(wb, cx, old, row, Action::Focus);
    request(wb, cx, old, row, Action::Click);
}

fn explorer_nodes(snap: &Snapshot) -> usize {
    snap.to_tree().nodes.iter().filter(|(_, n)| matches!(n.role(), Role::Tree | Role::TreeItem)).count()
}

#[gpui::test]
fn a_hidden_side_bar_takes_the_explorer_out_and_refuses_its_actions(cx: &mut TestAppContext) {
    let (wb, cx) = fixture("a11y-hidden-sidebar", cx);
    let shown = snapshot(&wb, cx);
    assert_eq!((shown.explorers.len(), explorer_nodes(&shown)), (1, 3));
    wb.update_in(cx, |wb, window, cx| wb.toggle_sidebar(&ToggleSidebar, window, cx));
    cx.run_until_parked();
    let hidden = snapshot(&wb, cx);
    assert!(hidden.explorers.is_empty());
    assert_eq!(explorer_nodes(&hidden), 0, "no tree and no rows");
    let before = explorer_state(&wb, cx);
    replay_explorer(&wb, cx, &shown);
    assert_eq!(explorer_state(&wb, cx), before, "nothing happens on the hidden Explorer");
    // Shown again: the tree is back, and its rows act.
    wb.update_in(cx, |wb, window, cx| wb.toggle_sidebar(&ToggleSidebar, window, cx));
    cx.run_until_parked();
    let again = snapshot(&wb, cx);
    assert_eq!(again.explorers.len(), 1);
    request(&wb, cx, &again, labelled(&again, "z.txt"), Action::Click);
    wait_ticking(cx, "the file", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).is_some_and(|p| p.ends_with("/z.txt")));
}

#[gpui::test]
fn another_view_in_the_side_bar_refuses_the_old_explorer_actions(cx: &mut TestAppContext) {
    let (wb, cx) = fixture("a11y-switched-view", cx);
    let shown = snapshot(&wb, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    let scm = snapshot(&wb, cx);
    assert!(scm.explorers.is_empty() && scm.scm.is_some());
    let before = explorer_state(&wb, cx);
    replay_explorer(&wb, cx, &shown);
    assert_eq!(explorer_state(&wb, cx), before);
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some("scm"), "the old action does not bring the Explorer back");
}

#[gpui::test]
fn zen_mode_hides_an_explorer_in_the_secondary_side_bar(cx: &mut TestAppContext) {
    let (wb, cx) = fixture("a11y-zen-secondary", cx);
    // The Explorer in the secondary side bar, Source Control in the primary one: both drawn, both in the tree.
    wb.update(cx, |wb, cx| wb.dock_view(Panel::Explorer, true, cx));
    cx.simulate_keystrokes("ctrl-shift-g");
    assert_eq!(wb.read_with(cx, |wb, _| (wb.chrome.panel, wb.chrome.secondary)), (Some(Panel::Scm), Some(Panel::Explorer)));
    let both = snapshot(&wb, cx);
    assert_eq!((both.explorers.len(), both.scm.is_some()), (1, true));
    // Zen mode draws neither side bar.
    wb.update_in(cx, |wb, window, cx| wb.toggle_zen(&ToggleZenMode, window, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.chrome.secondary.is_some()), "the secondary view is kept for later");
    assert!(!wb.read_with(cx, |wb, _| wb.view_shown(Panel::Explorer) || wb.view_shown(Panel::Scm)), "but neither is shown");
    let zen = snapshot(&wb, cx);
    assert!(zen.explorers.is_empty() && zen.scm.is_none(), "zen mode shows no side bar");
    let before = explorer_state(&wb, cx);
    replay_explorer(&wb, cx, &both);
    assert_eq!(explorer_state(&wb, cx), before);
    wb.update_in(cx, |wb, window, cx| wb.toggle_zen(&ToggleZenMode, window, cx));
    cx.run_until_parked();
    let back = snapshot(&wb, cx);
    assert_eq!((back.explorers.len(), back.scm.is_some()), (1, true), "back with the side bars");
}
