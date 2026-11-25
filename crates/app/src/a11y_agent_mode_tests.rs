//! Agent mode's controls in the accessibility tree: the switch of the project list, each project, the terminal tabs and the
//! buttons around them have names, and a press does what a click does.

use accesskit::{Action, ActionRequest, Node, NodeId, Role, TreeId};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_workbench_tests::setup;

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn find(snap: &Snapshot, role: Role, label: &str) -> Option<(NodeId, Node)> {
    snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == role && n.label() == Some(label))
}

fn get(snap: &Snapshot, role: Role, label: &str) -> (NodeId, Node) {
    find(snap, role, label).unwrap_or_else(|| panic!("{role:?} {label:?} is in the tree"))
}

fn press(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId) {
    let req = ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn agent_mode_has_named_buttons_and_a_press_does_what_a_click_does(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Toggle Agent Mode", window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    // The switch between the projects of this computer and of SSH hosts: the chosen one says so.
    let (_, all) = get(&snap, Role::Button, "All");
    assert_eq!(all.is_selected(), Some(true));
    assert_eq!(get(&snap, Role::Button, "SSH").1.is_selected(), Some(false));
    // This window's own project is listed, with what it needs from the person.
    get(&snap, Role::Button, "Codeyard, Idle");
    get(&snap, Role::Button, "Back to editor");

    // A press on "New terminal" opens one, as the plus does.
    let before = wb.read_with(cx, |wb, _| wb.terminal_count());
    let (plus, _) = get(&snap, Role::Button, "New terminal");
    press(&wb, cx, &snap, plus);
    assert_eq!(wb.read_with(cx, |wb, _| wb.terminal_count()), before + 1);

    // The scope switch is pressed from the tree too.
    let snap = snapshot(&wb, cx);
    let (ssh, _) = get(&snap, Role::Button, "SSH");
    press(&wb, cx, &snap, ssh);
    let snap = snapshot(&wb, cx);
    assert_eq!(get(&snap, Role::Button, "SSH").1.is_selected(), Some(true));
    assert_eq!(get(&snap, Role::Button, "All").1.is_selected(), Some(false));

    // And so is the way back to the editor.
    let (back, _) = get(&snap, Role::Button, "Back to editor");
    press(&wb, cx, &snap, back);
    assert!(find(&snapshot(&wb, cx), Role::Button, "Back to editor").is_none(), "the editor is back");
}
