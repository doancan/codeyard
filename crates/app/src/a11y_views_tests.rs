//! Views reached from the tree: Source Control's sync button, worktrees, graph and commit details; the Debug Console;
//! the editor's hover and signature popups. Each drives the real workbench through accessibility actions and checks
//! roles, states, frames and that an outdated action does nothing.

use accesskit::{Action, ActionData, ActionRequest, Node, NodeId, Role, Toggled, TreeId};
use accesskit_consumer::{FilterResult, Tree};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_integration_tests::{git_in, wait_ticking};
use crate::qa_workbench_tests::{fresh, open, setup};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn find(snap: &Snapshot, role: Role, label: &str) -> Option<(NodeId, Node)> {
    snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == role && n.label() == Some(label))
}

/// The node of `role` whose label starts with `prefix`.
fn starting(snap: &Snapshot, role: Role, prefix: &str) -> (NodeId, Node) {
    snap.to_tree()
        .nodes
        .into_iter()
        .find(|(_, n)| n.role() == role && n.label().is_some_and(|l| l.starts_with(prefix)))
        .unwrap_or_else(|| panic!("{role:?} {prefix:?}…"))
}

fn get(snap: &Snapshot, role: Role, label: &str) -> (NodeId, Node) {
    find(snap, role, label).unwrap_or_else(|| panic!("{role:?} {label:?} is in the tree"))
}

fn act(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId, action: Action, data: Option<ActionData>) {
    let req = ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

/// The node AccessKit's hit test finds at the centre of `id`'s frame is `id` itself.
fn hit_self(snap: &Snapshot, id: NodeId) {
    let update = snap.to_tree();
    let b = update.nodes.iter().find(|(i, _)| *i == id).and_then(|(_, n)| n.bounds()).unwrap_or_else(|| panic!("{id:?} has a frame"));
    let tree = Tree::new(update, true);
    let at = accesskit::Point::new((b.x0 + b.x1) / 2., (b.y0 + b.y1) / 2.);
    assert_eq!(tree.state().root().node_at_point(at, &|_| FilterResult::Include).map(|n| n.locate().0), Some(id));
}

/// As `hit_self`, at the left end of the frame (a section header holds its buttons on the right).
fn hit_left(snap: &Snapshot, id: NodeId) {
    let update = snap.to_tree();
    let b = update.nodes.iter().find(|(i, _)| *i == id).and_then(|(_, n)| n.bounds()).unwrap_or_else(|| panic!("{id:?} has a frame"));
    let tree = Tree::new(update, true);
    let at = accesskit::Point::new(b.x0 + 8., (b.y0 + b.y1) / 2.);
    assert_eq!(tree.state().root().node_at_point(at, &|_| FilterResult::Include).map(|n| n.locate().0), Some(id));
}

fn history(snap: &Snapshot) -> crate::scm::AccessHistory {
    snap.scm.as_ref().map(|s| s.view.history.clone()).unwrap_or_default()
}

/// A repository with two commits, an upstream one commit behind and a linked worktree; Source Control shown.
fn repo<'a>(name: &str, cx: &'a mut TestAppContext) -> Option<(std::path::PathBuf, Entity<Workbench>, &'a mut VisualTestContext)> {
    let dir = crate::tests::git_repo(name)?;
    let remote = fresh(&format!("{name}-remote"));
    git_in(&remote, &["init", "-q", "--bare"]);
    std::fs::write(dir.join("tracked.txt"), "one\n").unwrap();
    git_in(&dir, &["add", "tracked.txt"]);
    git_in(&dir, &["commit", "-qm", "Add tracked fixture"]);
    git_in(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
    let branch = String::from_utf8(std::process::Command::new("git").args(["branch", "--show-current"]).current_dir(&dir).output().unwrap().stdout).unwrap();
    git_in(&dir, &["push", "-q", "-u", "origin", branch.trim()]);
    std::fs::write(dir.join("tracked.txt"), "two\n").unwrap();
    git_in(&dir, &["commit", "-qam", "Change tracked fixture"]);
    let linked = dir.parent().unwrap().join(format!("{name}-linked"));
    let _ = std::fs::remove_dir_all(&linked);
    git_in(&dir, &["worktree", "add", "-q", "-b", "feature/a11y", linked.to_str().unwrap()]);
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    wait_ticking(cx, "the history", |cx| {
        let h = history(&snapshot(&wb, cx));
        h.graph.rows.len() >= 2 && h.worktrees.rows.len() == 2 && h.sync.is_some()
    });
    Some((dir, wb, cx))
}

/// The graph lists the commits as list items with their author, time and hash; a click selects one and its details
/// (message, files) follow, a file opens its diff, Open Changes opens the commit, More Actions… opens the commit's
/// menu. The search button opens a search field that filters as typed from the tree; the header folds the section.
#[gpui::test]
fn the_graph_and_commit_details_are_driven_from_the_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some((_dir, wb, cx)) = repo("a11y-graph", cx) else { return };
    let snap = snapshot(&wb, cx);
    let (header, node) = starting(&snap, Role::Button, "Graph (");
    assert_eq!(node.is_expanded(), Some(true));
    hit_left(&snap, header);
    let (row, item) = get(&snap, Role::ListItem, "Add tracked fixture");
    assert!(item.description().is_some_and(|d| d.contains(", ")), "author, time and hash: {:?}", item.description());
    assert!(item.supports_action(Action::ShowContextMenu));
    hit_self(&snap, row);
    act(&wb, cx, &snap, row, Action::Click, None);
    wait_ticking(cx, "the commit's details", |cx| history(&snapshot(&wb, cx)).detail.is_some_and(|d| !d.files.is_empty()));
    let snap = snapshot(&wb, cx);
    assert_eq!(get(&snap, Role::ListItem, "Add tracked fixture").1.is_selected(), Some(true));
    let detail = history(&snap).detail.unwrap();
    assert_eq!(detail.files, [("tracked.txt".to_string(), "Added".to_string())]);
    let (file, _) = get(&snap, Role::Button, "tracked.txt");
    act(&wb, cx, &snap, file, Action::Click, None);
    wait_ticking(cx, "the commit's diff", |cx| snapshot(&wb, cx).active_tab.is_none());

    // More Actions… on the row opens the commit's menu in the tree.
    let snap = snapshot(&wb, cx);
    let (row, item) = get(&snap, Role::ListItem, "Change tracked fixture");
    let more = item.children()[1];
    act(&wb, cx, &snap, more, Action::Click, None);
    let menu = snapshot(&wb, cx);
    assert!(menu.to_tree().nodes.iter().any(|(_, n)| n.role() == Role::Menu), "the commit's menu is open");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let _ = row;

    // Search: the button is a toggle; the field filters the rows as typed from the tree.
    let snap = snapshot(&wb, cx);
    let (search, node) = get(&snap, Role::Button, "Search commits");
    assert_eq!(node.toggled(), Some(Toggled::False));
    act(&wb, cx, &snap, search, Action::Click, None);
    let snap = snapshot(&wb, cx);
    assert_eq!(get(&snap, Role::Button, "Search commits").1.toggled(), Some(Toggled::True));
    let (field, _) = get(&snap, Role::SearchInput, "Search commits");
    assert_eq!(snap.to_tree().focus, field, "the field has the keyboard");
    act(&wb, cx, &snap, field, Action::SetValue, Some(ActionData::Value("Change".into())));
    let snap = snapshot(&wb, cx);
    assert!(find(&snap, Role::ListItem, "Change tracked fixture").is_some());
    assert!(find(&snap, Role::ListItem, "Add tracked fixture").is_none(), "filtered out");

    // The header folds the section; a press from the old tree (it showed it open) does not unfold it again.
    let (header, _) = starting(&snap, Role::Button, "Graph (");
    act(&wb, cx, &snap, header, Action::Click, None);
    act(&wb, cx, &snap, header, Action::Click, None);
    let folded = snapshot(&wb, cx);
    assert_eq!(starting(&folded, Role::Button, "Graph (").1.is_expanded(), Some(false));
    assert!(find(&folded, Role::ListItem, "Change tracked fixture").is_none());
}

/// Sync and the Graph's remote buttons: the commit to push is said on Push and on the Sync button, and pressing Sync
/// from the tree pushes it. Worktrees: the current checkout is selected and disabled, the other opens from the tree.
#[gpui::test]
fn sync_and_worktrees_are_driven_from_the_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some((dir, wb, cx)) = repo("a11y-sync", cx) else { return };
    let snap = snapshot(&wb, cx);
    let h = history(&snap);
    let label = h.sync.clone().unwrap();
    assert!(label.starts_with("Sync Changes"), "{label}");
    let (push, node) = get(&snap, Role::Button, "Push");
    assert_eq!(node.description(), Some("1 commit"));
    hit_self(&snap, push);
    for name in ["Pull", "Fetch from all remotes", "Reload history", "More Actions…"] {
        assert!(find(&snap, Role::Button, name).is_some(), "{name}");
    }
    let (sync, _) = get(&snap, Role::Button, &label);
    hit_self(&snap, sync);
    act(&wb, cx, &snap, sync, Action::Click, None);
    wait_ticking(cx, "the push", |cx| history(&snapshot(&wb, cx)).sync.is_none());

    // Worktrees: two rows, the current one selected and disabled; the linked one opens.
    let snap = snapshot(&wb, cx);
    let (header, node) = get(&snap, Role::Button, "Worktrees (2)");
    assert_eq!(node.is_expanded(), Some(true));
    hit_left(&snap, header);
    let rows: Vec<(NodeId, Node)> = snap
        .to_tree()
        .nodes
        .into_iter()
        .filter(|(_, n)| n.role() == Role::ListItem && n.description().is_some_and(|d| d.contains('/') || d.contains('\\')))
        .collect();
    let current = rows.iter().find(|(_, n)| n.is_selected() == Some(true)).expect("the current checkout");
    assert!(current.1.is_disabled());
    let (linked, node) = rows.iter().find(|(_, n)| n.label() == Some("feature/a11y")).expect("the linked checkout").clone();
    assert!(!node.is_disabled());
    hit_self(&snap, linked);
    act(&wb, cx, &snap, linked, Action::Click, None);
    let path = dir.parent().unwrap().join("a11y-sync-linked");
    wait_ticking(cx, "the linked checkout", |cx| {
        wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|s| std::path::Path::new(&s.root).canonicalize().ok() == path.canonicalize().ok()))
    });
}

/// The Debug Console: a group with the output as a polite log and the expression field; text set from the tree and
/// Evaluate run it as Enter does (the field empties, the output shows the input and the answer). Frames on both.
#[gpui::test]
fn the_debug_console_is_read_and_typed_from_the_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("a11y-console");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update(cx, |wb, cx| {
        wb.push_paused_session_for_test("app", cx);
        wb.chrome.panel_open = true;
        wb.chrome.bottom = Bottom::Debug;
        cx.notify();
    });
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let (_, group) = get(&snap, Role::Group, "Debug Console");
    assert_eq!(group.description(), Some("app"));
    let (log, node) = get(&snap, Role::Log, "Output of app");
    assert_eq!(node.live(), Some(Live::Polite));
    let (field, _) = get(&snap, Role::TextInput, "Evaluate an expression (↵)");
    let (evaluate, node) = get(&snap, Role::Button, "Evaluate");
    assert!(node.is_disabled(), "nothing to evaluate yet");
    hit_self(&snap, field);
    hit_self(&snap, log);
    act(&wb, cx, &snap, field, Action::Focus, None);
    assert_eq!(snapshot(&wb, cx).to_tree().focus, field);
    act(&wb, cx, &snap, field, Action::SetValue, Some(ActionData::Value("1 + 1".into())));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.console_input_for_test(cx)), "1 + 1");
    // From the tree that showed an empty field, Evaluate does nothing.
    act(&wb, cx, &snap, evaluate, Action::Click, None);
    assert_eq!(wb.read_with(cx, |wb, cx| wb.console_input_for_test(cx)), "1 + 1");
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, evaluate, Action::Click, None);
    assert_eq!(wb.read_with(cx, |wb, cx| wb.console_input_for_test(cx)), "", "the field empties as after Enter");
    let value = get(&snapshot(&wb, cx), Role::Log, "Output of app").1.value().map(str::to_string).unwrap_or_default();
    assert!(value.contains("> 1 + 1") && value.contains("No active debug session."), "{value}");
}

/// The hover: `ShowTooltip` on the editor shows it at the caret, as "Editor: Show Hover"; it is a polite tooltip under
/// the editor with its text and frame, and `HideTooltip` closes it. The signature help is a tooltip whose description
/// names the parameter being typed.
#[gpui::test]
fn hover_and_signature_popups_are_read_from_the_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("a11y-hover");
    std::fs::write(dir.join("notes.md"), "This sentense has a mistake.\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("notes.md"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_ticking(cx, "the speller", |cx| ed.read_with(cx, |e, _| e.has_speller()));
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(0, 7, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let editor = snap.to_tree().nodes.into_iter().find(|(id, _)| *id == EDITOR).unwrap().1;
    assert!(editor.supports_action(Action::ShowTooltip));
    act(&wb, cx, &snap, EDITOR, Action::ShowTooltip, None);
    wait_ticking(cx, "the hover", |cx| snapshot(&wb, cx).popups.is_some_and(|p| p.hover.is_some()));
    let snap = snapshot(&wb, cx);
    let (hover, node) = get(&snap, Role::Tooltip, "Hover");
    assert_eq!(node.live(), Some(Live::Polite));
    let (line_id, line) = snap.to_tree().nodes.into_iter().find(|(id, _)| node.children().contains(id)).unwrap();
    assert!(line.label().is_some_and(|l| l.contains("sentense")), "{:?}", line.label());
    assert!(node.bounds().is_some(), "{hover:?} has its frame");
    hit_self(&snap, line_id);
    act(&wb, cx, &snap, EDITOR, Action::HideTooltip, None);
    assert!(find(&snapshot(&wb, cx), Role::Tooltip, "Hover").is_none());

    ed.update(cx, |e, cx| e.set_signature_for_test("fn add(a: i32, b: i32)", Some(15..21), cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let (sig, node) = get(&snap, Role::Tooltip, "Signature help");
    assert_eq!(node.value(), Some("fn add(a: i32, b: i32)"));
    assert_eq!(node.description(), Some("Current parameter: b: i32"));
    hit_self(&snap, sig);
}
