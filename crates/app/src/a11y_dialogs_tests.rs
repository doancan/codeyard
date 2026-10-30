use accesskit::{Action, ActionRequest, NodeId, Role, TreeId};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::market_tests::{dir, registry, setup};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn act(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId, action: Action) {
    let request = ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&request, snap, window, cx));
    cx.run_until_parked();
}

fn review(wb: &Entity<Workbench>, cx: &mut VisualTestContext, spec: &str) {
    wb.update_in(cx, |wb, window, cx| wb.review_registry_extension_for_test(spec, window, cx));
    cx.run_until_parked();
}

fn sheet(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<(String, &'static str)> {
    wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).map(|(id, _, _, focus)| (id, focus))
}

/// AC1/AC2/AC6: the consent sheet is a modal alert dialog that says what the package is, where it comes from and what
/// it may do; the keyboard lands on its button, Escape and Cancel give the keyboard back to where it was, and a button
/// of an older sheet answers nothing.
#[gpui::test]
fn the_consent_sheet_is_a_dialog_that_returns_the_keyboard(cx: &mut TestAppContext) {
    let root = dir("a11y-consent");
    let (_r, _fetch) = registry(&root, "consent", crate::ext_registry::now());
    let (wb, cx) = setup(cx, &root.join("cfg"));
    // The keyboard is in the Extensions search field when the sheet opens.
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, EXT_SEARCH, Action::Focus);
    assert_eq!(snapshot(&wb, cx).to_tree().focus, EXT_SEARCH);

    review(&wb, cx, "you.dawn");
    let snap = snapshot(&wb, cx);
    let consent = snap.views.consent.as_ref().expect("the sheet is in the tree");
    assert_eq!(consent.title, "Install Dawn?");
    assert!(consent.lines.iter().any(|l| l == "Permissions: none."), "{:?}", consent.lines);
    assert!(consent.lines.iter().any(|l| l == "Color theme: Dawn"), "{:?}", consent.lines);
    let update = snap.to_tree();
    assert_eq!(update.focus, CONSENT_INSTALL, "the keyboard is on Install (not a High-risk set)");
    let tree = accesskit_consumer::Tree::new(update, true);
    let state = tree.state();
    let dialog = state.root().children().find(|n| n.role() == Role::AlertDialog).expect("an alert dialog");
    assert_eq!(dialog.label().as_deref(), Some("Install Dawn?"));
    assert!(dialog.is_modal());
    assert!(dialog.description().is_some_and(|d| d.starts_with("From the consent registry")), "{:?}", dialog.description());
    let buttons: Vec<_> = dialog.children().filter(|n| n.role() == Role::Button).map(|n| n.label().unwrap_or_default()).collect();
    assert_eq!(buttons, ["Install", "Cancel"]);
    let focus = state.focus().expect("a focused node");
    assert_eq!((focus.role(), focus.label().as_deref()), (Role::Button, Some("Install")));

    // Focus moves to Cancel as Tab would; Escape answers Cancel and the search field has the keyboard again.
    act(&wb, cx, &snap, CONSENT_CANCEL, Action::Focus);
    assert_eq!(sheet(&wb, cx), Some(("you.dawn".into(), "cancel")));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(sheet(&wb, cx), None, "Escape cancels");
    let after = snapshot(&wb, cx);
    assert!(after.views.consent.is_none() && !after.to_tree().nodes.iter().any(|(_, n)| n.role() == Role::AlertDialog));
    assert_eq!(after.to_tree().focus, EXT_SEARCH, "the keyboard is back where it was");

    // A sheet for another package: the old tree's Install installs nothing and leaves the new sheet open.
    review(&wb, cx, "acme.forest");
    act(&wb, cx, &snap, CONSENT_INSTALL, Action::Click);
    assert_eq!(sheet(&wb, cx).map(|s| s.0), Some("acme.forest".into()), "an outdated Install answers nothing");

    // Its own Install installs it, as a click does, and gives the keyboard back.
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, CONSENT_INSTALL, Action::Click);
    assert_eq!(sheet(&wb, cx), None);
    assert!(wb.read_with(cx, |wb, _| wb.themes_for_test()).iter().any(|(t, _)| t.starts_with("ext:acme.forest/")));
    assert_eq!(snapshot(&wb, cx).to_tree().focus, EXT_SEARCH);
}

/// Cancel from the tree closes the sheet like a click on it and returns the keyboard.
#[gpui::test]
fn the_consent_sheet_cancel_button_acts_like_a_click(cx: &mut TestAppContext) {
    let root = dir("a11y-consent-cancel");
    let (_r, _fetch) = registry(&root, "cancel", crate::ext_registry::now());
    let (wb, cx) = setup(cx, &root.join("cfg"));
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, EXT_SEARCH, Action::Focus);
    review(&wb, cx, "you.dawn");
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, CONSENT_CANCEL, Action::Click);
    assert_eq!(sheet(&wb, cx), None);
    assert!(!wb.read_with(cx, |wb, _| wb.themes_for_test()).iter().any(|(t, _)| t.starts_with("ext:you.dawn/")), "nothing installed");
    assert_eq!(snapshot(&wb, cx).to_tree().focus, EXT_SEARCH);
}
