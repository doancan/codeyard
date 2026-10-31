//! Source Control in the accessibility tree of a real workbench and repository: the rows and their actions (open,
//! stage/unstage, discard behind the question), the focus and keyboard cursor, the bounded range of a large status,
//! stale actions after a refresh or another repository, and a Source Control window of its own.

use accesskit::{Action, ActionRequest, NodeId, Role, TreeId};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_integration_tests::{git_in, wait_ticking};
use crate::qa_workbench_tests::{open, setup};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn node(snap: &Snapshot, label: &str) -> Option<(NodeId, Node)> {
    snap.to_tree().nodes.into_iter().find(|(_, n)| n.label() == Some(label))
}

fn act_on(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId, action: Action) {
    let req = ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

fn act(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, label: &str, action: Action) {
    let (id, _) = node(snap, label).unwrap_or_else(|| panic!("missing node: {label}"));
    act_on(wb, cx, snap, id, action);
}

/// The unstaged/staged state of `path` in the exposed rows.
fn row(snap: &Snapshot, path: &str) -> Vec<bool> {
    snap.scm.iter().flat_map(|s| &s.view.groups).flat_map(|g| &g.files).filter(|f| f.change.path == path).map(|f| f.staged).collect()
}

fn idle(wb: &Entity<Workbench>, cx: &mut VisualTestContext, what: &str, done: impl Fn(&Snapshot) -> bool) {
    wait_ticking(cx, what, |cx| {
        let snap = snapshot(wb, cx);
        snap.scm.as_ref().is_some_and(|s| !s.view.busy) && done(&snap)
    });
}

/// A repository with a committed file changed since and an untracked file, Source Control shown.
fn repo<'a>(name: &str, cx: &'a mut TestAppContext) -> Option<(std::path::PathBuf, Entity<Workbench>, &'a mut VisualTestContext)> {
    let dir = crate::tests::git_repo(name)?;
    std::fs::write(dir.join("tracked.txt"), "before\n").unwrap();
    git_in(&dir, &["add", "tracked.txt"]);
    git_in(&dir, &["commit", "-qm", "Add tracked fixture"]);
    std::fs::write(dir.join("tracked.txt"), "after\n").unwrap();
    std::fs::write(dir.join("untracked.txt"), "new\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    idle(&wb, cx, "the changes", |s| row(s, "tracked.txt") == [false] && row(s, "untracked.txt") == [false]);
    Some((dir, wb, cx))
}

#[gpui::test]
fn scm_rows_expose_their_state_and_actions(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some((_dir, wb, cx)) = repo("a11y-scm-rows", cx) else { return };
    let snap = snapshot(&wb, cx);
    let tree = snap.to_tree();
    accesskit_consumer::Tree::new(tree.clone(), true);
    let (_, list) = tree.nodes.iter().find(|(id, _)| *id == SCM_CHANGES).expect("the change list");
    assert_eq!((list.role(), list.label()), (Role::List, Some("Changed files")));
    assert_eq!((list.scroll_y(), list.scroll_y_max()), (Some(0.), Some(0.)));
    assert!(list.supports_action(Action::ScrollDown) && list.supports_action(Action::Focus));
    let (_, header) = node(&snap, "Changes (2)").expect("the group header with its count");
    assert_eq!(header.is_expanded(), Some(true));
    let (_, file) = node(&snap, "tracked.txt").expect("the file row");
    assert_eq!((file.role(), file.description()), (Role::Button, Some("Modified")));
    assert_eq!((file.position_in_set(), file.size_of_set()), (Some(1), Some(2)));
    assert!(file.supports_action(Action::Click) && file.supports_action(Action::Focus));
    assert!(node(&snap, "Stage tracked.txt").is_some_and(|(_, n)| n.supports_action(Action::Click)));
    assert!(node(&snap, "Discard changes in untracked.txt").is_some_and(|(_, n)| n.supports_action(Action::Click)));
    // Commit state: nothing staged, so Commit All for the tracked change.
    let (_, commit) = tree.nodes.iter().find(|(id, _)| *id == SCM_COMMIT).unwrap();
    assert_eq!(commit.label(), Some("Commit All (1)"));

    // Clicking the row opens its diff.
    act(&wb, cx, &snap, "tracked.txt", Action::Click);
    wait_ticking(cx, "the diff", |cx| wb.read_with(cx, |wb, cx| wb.diff_view().is_some_and(|d| d.read(cx).path == "tracked.txt")));

    // Staged: the row moves to Staged Changes, with Unstage and no Discard.
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, "Stage tracked.txt", Action::Click);
    idle(&wb, cx, "staging", |s| row(s, "tracked.txt") == [true]);
    let staged = snapshot(&wb, cx);
    assert!(node(&staged, "Unstage tracked.txt").is_some());
    assert!(node(&staged, "Discard changes in tracked.txt").is_none(), "a staged row is not discarded");
    assert!(node(&staged, "Staged Changes (1)").is_some());
    assert_eq!(staged.to_tree().nodes.iter().find(|(id, _)| *id == SCM_COMMIT).unwrap().1.label(), Some("Commit (1)"));

    // A group folds once: the same press replayed from the old tree does not unfold it again.
    act(&wb, cx, &staged, "Changes (1)", Action::Click);
    act(&wb, cx, &staged, "Changes (1)", Action::Click);
    let folded = snapshot(&wb, cx);
    assert_eq!(node(&folded, "Changes (1)").and_then(|(_, n)| n.is_expanded()), Some(false));
    assert!(node(&folded, "untracked.txt").is_none(), "a folded group shows no rows");
}

#[gpui::test]
fn scm_discard_asks_first_and_stale_rows_are_refused(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some((dir, wb, cx)) = repo("a11y-scm-discard", cx) else { return };
    let before = snapshot(&wb, cx);
    // The question comes first; Cancel keeps the file.
    act(&wb, cx, &before, "Discard changes in untracked.txt", Action::Click);
    assert!(cx.has_pending_prompt(), "discarding asks first");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(dir.join("untracked.txt").exists());
    act(&wb, cx, &before, "Discard changes in untracked.txt", Action::Click);
    cx.simulate_prompt_answer("Discard");
    idle(&wb, cx, "the discard", |s| row(s, "untracked.txt").is_empty());
    assert!(!dir.join("untracked.txt").exists());

    // The old tree's rows: the file is gone, so its actions do nothing.
    act(&wb, cx, &before, "Discard changes in untracked.txt", Action::Click);
    assert!(!cx.has_pending_prompt(), "a file that is no longer changed is not acted on");
    act(&wb, cx, &before, "Stage untracked.txt", Action::Click);
    assert!(git_in(&dir, &["diff", "--cached", "--name-only"]).trim().is_empty());

    // Staged meanwhile: the old Discard of the unstaged row does not ask about the staged file.
    act(&wb, cx, &before, "Stage tracked.txt", Action::Click);
    idle(&wb, cx, "staging", |s| row(s, "tracked.txt") == [true]);
    act(&wb, cx, &before, "Discard changes in tracked.txt", Action::Click);
    assert!(!cx.has_pending_prompt(), "the row changed state");
    assert_eq!(std::fs::read_to_string(dir.join("tracked.txt")).unwrap(), "after\n");

    // A hidden Source Control does not act either.
    let staged = snapshot(&wb, cx);
    wb.update_in(cx, |wb, window, cx| wb.show_explorer(&ShowExplorer, window, cx));
    cx.run_until_parked();
    assert!(snapshot(&wb, cx).scm.is_none());
    act(&wb, cx, &staged, "Unstage tracked.txt", Action::Click);
    assert_eq!(git_in(&dir, &["diff", "--cached", "--name-only"]).trim(), "tracked.txt");
}

#[gpui::test]
fn scm_actions_of_another_repository_are_refused(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some((first, wb, cx)) = repo("a11y-scm-switch-a", cx) else { return };
    let old = snapshot(&wb, cx);
    // Another repository with the same file names.
    let Some(second) = crate::tests::git_repo("a11y-scm-switch-b") else { return };
    std::fs::write(second.join("tracked.txt"), "b\n").unwrap();
    open(&wb, &second, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    idle(&wb, cx, "the other repository", |s| {
        s.scm.as_ref().is_some_and(|s| s.view.root != old.scm.as_ref().unwrap().view.root) && row(s, "tracked.txt") == [false]
    });
    act(&wb, cx, &old, "Stage tracked.txt", Action::Click);
    act(&wb, cx, &old, "Discard changes in tracked.txt", Action::Click);
    assert!(!cx.has_pending_prompt());
    assert!(git_in(&second, &["diff", "--cached", "--name-only"]).trim().is_empty(), "nothing staged in the new repository");
    assert!(git_in(&first, &["diff", "--cached", "--name-only"]).trim().is_empty(), "nor in the old one");
    assert!(second.join("tracked.txt").exists());
}

#[gpui::test]
fn scm_focus_follows_the_cursor_row_and_the_keyboard(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some((dir, wb, cx)) = repo("a11y-scm-focus", cx) else { return };
    let snap = snapshot(&wb, cx);
    let (tracked, _) = node(&snap, "tracked.txt").unwrap();
    act_on(&wb, cx, &snap, tracked, Action::Focus);
    let snap = snapshot(&wb, cx);
    assert_eq!(snap.focus, FocusSnap::ScmChanges);
    assert_eq!(snap.to_tree().focus, tracked, "the focused row");
    // The keyboard moves the cursor and the accessibility focus with it.
    cx.simulate_keystrokes("down");
    let snap = snapshot(&wb, cx);
    assert_eq!(Some(snap.to_tree().focus), node(&snap, "untracked.txt").map(|(id, _)| id));
    cx.simulate_keystrokes("home");
    let snap = snapshot(&wb, cx);
    assert_eq!(Some(snap.to_tree().focus), node(&snap, "Changes (2)").map(|(id, _)| id));
    // Space on a file stages it; the cursor stays on the file, now in Staged Changes.
    cx.simulate_keystrokes("down space");
    idle(&wb, cx, "staging", |s| row(s, "tracked.txt") == [true]);
    assert_eq!(git_in(&dir, &["diff", "--cached", "--name-only"]).trim(), "tracked.txt");
    // Focus on the list puts the focus back on its cursor row; the message box takes it away.
    let snap = snapshot(&wb, cx);
    act_on(&wb, cx, &snap, SCM_MESSAGE, Action::Focus);
    assert_eq!(snapshot(&wb, cx).focus, FocusSnap::ScmMessage);
    // Tab goes from the message box to the list, and Shift+Tab back.
    cx.simulate_keystrokes("tab");
    assert_eq!(snapshot(&wb, cx).focus, FocusSnap::ScmChanges);
    cx.simulate_keystrokes("shift-tab");
    assert_eq!(snapshot(&wb, cx).focus, FocusSnap::ScmMessage);
    act_on(&wb, cx, &snap, SCM_CHANGES, Action::Focus);
    let snap = snapshot(&wb, cx);
    assert_eq!(snap.focus, FocusSnap::ScmChanges);
    assert!(snap.scm.as_ref().unwrap().view.cursor.is_some());
    // Enter on a file opens its diff.
    let (untracked, _) = node(&snap, "untracked.txt").unwrap();
    act_on(&wb, cx, &snap, untracked, Action::Focus);
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "the diff", |cx| wb.read_with(cx, |wb, cx| wb.diff_view().is_some_and(|d| d.read(cx).path == "untracked.txt")));
}

#[gpui::test]
fn scm_large_status_is_a_bounded_range_that_scrolls(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(dir) = crate::tests::git_repo("a11y-scm-range") else { return };
    for i in 0..1_200 {
        std::fs::write(dir.join(format!("new-{i:04}.txt")), "").unwrap();
    }
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    idle(&wb, cx, "the changes", |s| s.scm.as_ref().is_some_and(|s| s.view.lines_total >= 1_201));
    let snap = snapshot(&wb, cx);
    let view = &snap.scm.as_ref().unwrap().view;
    assert_eq!((view.lines_first, view.lines_shown), (0, crate::scm::ACCESS_LINES));
    let tree = snap.to_tree();
    accesskit_consumer::Tree::new(tree.clone(), true);
    let rows = tree.nodes.iter().filter(|(id, _)| (SCM_FILE..SCM_FILE + BLOCK).contains(&id.0) && (id.0 - SCM_FILE).is_multiple_of(4)).count();
    assert_eq!(rows, crate::scm::ACCESS_LINES - 1, "the header and the files of one range");
    // The last row by keyboard: the range moves to it, and the place is in the whole group.
    let (first, _) = node(&snap, "new-0000.txt").unwrap();
    act_on(&wb, cx, &snap, first, Action::Focus);
    cx.simulate_keystrokes("end");
    let snap = snapshot(&wb, cx);
    let view = &snap.scm.as_ref().unwrap().view;
    assert_eq!(view.lines_first + view.lines_shown, view.lines_total, "the range ends with the list");
    let (focus, last) = node(&snap, "new-1199.txt").unwrap();
    assert_eq!(snap.to_tree().focus, focus);
    assert_eq!((last.position_in_set(), last.size_of_set()), (Some(1_200), Some(1_200)));
    // Scrolling up by pages: the range follows the rows on screen.
    for _ in 0..60 {
        let snap = snapshot(&wb, cx);
        act_on(&wb, cx, &snap, SCM_CHANGES, Action::ScrollUp);
    }
    let snap = snapshot(&wb, cx);
    assert_eq!(snap.scm.as_ref().unwrap().view.lines_first, 0);
    assert_eq!(snap.to_tree().focus, SCM_CHANGES, "the cursor row is outside the range");
}

#[gpui::test]
fn scm_in_its_own_window_keeps_its_tree_and_focus(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some((_dir, wb, vcx)) = repo("a11y-scm-window", cx) else { return };
    wb.update_in(vcx, |wb, window, cx| wb.view_into_new_window(window, cx));
    vcx.run_until_parked();
    assert!(wb.read_with(vcx, |wb, _| wb.in_view_window(Panel::Scm)));
    let handle = wb.read_with(vcx, |wb, _| wb.chrome.view_windows.iter().find(|w| w.view.panel == Panel::Scm).map(|w| w.window)).unwrap();
    let mut win = VisualTestContext::from_window(handle.into(), vcx);
    let root = handle.update(&mut win, |_, _, cx| cx.entity()).unwrap();
    let take = |win: &mut VisualTestContext| win.update(|window, cx| (windows_access::VIEW_WINDOW.snapshot)(root.clone().into(), window, cx)).unwrap();
    let snap = take(&mut win);
    assert!(snap.scm.is_some(), "the window's tree holds Source Control");
    let main = vcx.update(|window, cx| (windows_access::WORKBENCH.snapshot)(wb.clone().into(), window, cx)).unwrap();
    assert!(main.scm.is_none(), "and the main window's does not");
    let (id, _) = node(&snap, "tracked.txt").unwrap();
    let req = ActionRequest { action: Action::Focus, target_tree: TreeId::ROOT, target_node: id, data: None };
    win.update(|window, cx| (windows_access::VIEW_WINDOW.action)(root.clone().into(), &req, &snap, window, cx));
    win.run_until_parked();
    let snap = take(&mut win);
    assert_eq!((snap.focus, snap.to_tree().focus), (FocusSnap::ScmChanges, id));
    let main = vcx.update(|window, cx| (windows_access::WORKBENCH.snapshot)(wb.clone().into(), window, cx)).unwrap();
    assert_ne!(main.focus, FocusSnap::ScmChanges);
}
