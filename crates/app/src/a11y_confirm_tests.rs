//! Confirmations and cancellations through the accessibility tree: what a screen reader user reaches a question from
//! (a row's menu, a button, a command), what Cancel and the confirming button leave behind, where the keyboard and the
//! tree's focus are afterwards, and that a failure is announced at once. The questions themselves are the system's
//! (a sheet on macOS, a task dialog on Windows), which their screen readers read; the test platform answers them.

use accesskit::{Action, ActionRequest, Live, Node, NodeId, Role, TreeId};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_workbench_tests::{fresh, open, setup};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn labelled(snap: &Snapshot, role: Role, label: &str) -> Option<(NodeId, Node)> {
    snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == role && n.label() == Some(label))
}

fn act(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId, action: Action) {
    let req = ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

/// The Explorer row of `name` and its "More Actions…" button.
fn explorer_row(snap: &Snapshot, name: &str) -> (NodeId, NodeId) {
    let update = snap.to_tree();
    let (row, node) = update.nodes.iter().find(|(_, n)| n.role() == Role::TreeItem && n.label() == Some(name)).cloned().expect("the row");
    let more = *node.children().iter().find(|id| update.nodes.iter().any(|(i, n)| i == *id && n.label() == Some("More Actions…"))).expect("its menu button");
    (row, more)
}

/// Delete from an Explorer row's menu: the menu opens from the tree (the row's button or `ShowContextMenu`), Delete in
/// it asks first; Cancel keeps the file and gives the keyboard back to the row, Delete removes it and leaves the
/// keyboard in the Explorer. A delete that fails is announced at once.
#[gpui::test]
fn explorer_delete_from_the_menu_asks_and_returns_the_keyboard(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("a11y-confirm-delete");
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    std::fs::write(dir.join("b.txt"), "b\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let snap = snapshot(&wb, cx);
    let (row, more) = explorer_row(&snap, "a.txt");
    act(&wb, cx, &snap, row, Action::Focus);
    assert_eq!(snapshot(&wb, cx).to_tree().focus, row, "the keyboard is on the row");

    for (open_with, answer) in [(more, "Cancel"), (row, "Delete")] {
        let snap = snapshot(&wb, cx);
        act(&wb, cx, &snap, open_with, if open_with == row { Action::ShowContextMenu } else { Action::Click });
        let menu = snapshot(&wb, cx);
        let (delete, _) = labelled(&menu, Role::MenuItem, "Delete").expect("the row's menu is in the tree");
        assert_eq!(
            menu.to_tree().nodes.iter().find(|(_, n)| n.role() == Role::Menu).map(|(_, n)| n.label().map(str::to_string)),
            Some(Some("Context menu".into()))
        );
        act(&wb, cx, &menu, delete, Action::Click);
        assert!(cx.has_pending_prompt(), "deleting asks first");
        cx.simulate_prompt_answer(answer);
        cx.run_until_parked();
        let after = snapshot(&wb, cx);
        if answer == "Cancel" {
            assert!(dir.join("a.txt").exists(), "Cancel keeps the file");
            assert_eq!(after.to_tree().focus, row, "Cancel gives the keyboard back to the row");
        } else {
            wait(cx, "the delete", |_| !dir.join("a.txt").exists());
            let after = snapshot(&wb, cx);
            assert!(matches!(after.focus, FocusSnap::Explorer(0)), "the keyboard stays in the Explorer: {:?}", after.focus);
            assert!(labelled(&after, Role::TreeItem, "a.txt").is_none());
        }
    }

    // A failure is an assertive alert: the file went away behind our back and its folder can't be written to.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let snap = snapshot(&wb, cx);
        let (row, _) = explorer_row(&snap, "b.txt");
        act(&wb, cx, &snap, row, Action::ShowContextMenu);
        let menu = snapshot(&wb, cx);
        let (delete, _) = labelled(&menu, Role::MenuItem, "Delete").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        act(&wb, cx, &menu, delete, Action::Click);
        cx.simulate_prompt_answer("Delete");
        cx.run_until_parked();
        let alert = |cx: &mut VisualTestContext| snapshot(&wb, cx).to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Alert);
        wait(cx, "the error", |cx| alert(cx).is_some());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(alert(cx).unwrap().1.live(), Some(Live::Assertive));
        assert!(dir.join("b.txt").exists());
    }
}

/// Closing an editor with unsaved changes: Cancel keeps it and the keyboard in it; Save writes it and closes it.
#[gpui::test]
fn closing_an_unsaved_editor_asks_and_cancel_keeps_the_keyboard(cx: &mut TestAppContext) {
    let dir = fresh("a11y-confirm-close");
    std::fs::write(dir.join("a.txt"), "disk\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("a.txt"), cx);
    cx.simulate_input("new ");
    assert_eq!(snapshot(&wb, cx).to_tree().focus, EDITOR);
    close_editor(&wb, cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    assert!(snap.editor.as_ref().is_some_and(|e| e.dirty), "still open, still unsaved");
    assert_eq!(snap.to_tree().focus, EDITOR, "the keyboard is back in the editor");
    close_editor(&wb, cx);
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "new disk\n");
    assert!(snapshot(&wb, cx).editor.is_none(), "closed");
}

/// Applying the Rust setup to settings.jsonc asks first; Cancel leaves the file as it was and the keyboard where it was.
#[gpui::test]
fn a_settings_change_asks_and_cancel_changes_nothing(cx: &mut TestAppContext) {
    let cfg = fresh("a11y-confirm-settings");
    std::fs::write(cfg.join("settings.jsonc"), "{}\n").unwrap();
    let dir = crate::qa_workbench_tests::project("a11y-confirm-settings-project");
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &cfg);
    open(&wb, &dir.join("src/main.rs"), cx);
    let focus = snapshot(&wb, cx).to_tree().focus;
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Apply Rust Setup…", window, cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "the change is shown and asked about first");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap(), "{}\n");
    assert_eq!(snapshot(&wb, cx).to_tree().focus, focus, "the keyboard is where it was");
}

fn wait(cx: &mut VisualTestContext, what: &str, done: impl FnMut(&mut VisualTestContext) -> bool) {
    crate::qa_integration_tests::wait_ticking(cx, what, done);
}

/// "Editor: Close", as its key does on every platform.
fn close_editor(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Close", window, cx));
}
