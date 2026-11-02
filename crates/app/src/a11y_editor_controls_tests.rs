//! The editor's drawn controls in the tree (`a11y_controls.rs`): gutter breakpoints, fold buttons and Git marks,
//! merge-conflict links, Send Request, the color picker's channels as sliders, and the terminal's find bar and
//! history. Expand and Collapse as UI Automation sends them.

use accesskit::{Action, ActionData, ActionRequest, Node, NodeId, Role, TreeId};
use accesskit_consumer::{FilterResult, Tree};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_integration_tests::wait_ticking;
use crate::qa_workbench_tests::{fresh, open, setup};

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn find(snap: &Snapshot, role: Role, pred: impl Fn(&str) -> bool) -> Option<(NodeId, Node)> {
    snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == role && n.label().is_some_and(&pred))
}

fn get(snap: &Snapshot, role: Role, label: &str) -> (NodeId, Node) {
    find(snap, role, |l| l == label).unwrap_or_else(|| panic!("{role:?} {label:?} is in the tree"))
}

fn act(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId, action: Action, data: Option<ActionData>) {
    let req = ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data };
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

#[gpui::test]
fn gutter_breakpoints_and_folds(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("a11y-editor-gutter");
    std::fs::write(dir.join("main.rs"), "fn a() {\n    x();\n}\nfn b() {}\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("main.rs"), cx);
    cx.simulate_keystrokes("down");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Debug: Toggle Breakpoint", window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let (bp, _) = get(&snap, Role::Button, "Breakpoint on line 2");
    hit(&snap, bp);
    act(&wb, cx, &snap, bp, Action::Click, None);
    assert!(find(&snapshot(&wb, cx), Role::Button, |l| l == "Breakpoint on line 2").is_none(), "removed as a click on the dot does");

    // A fold button: Collapse (as UI Automation sends it) folds; the button then unfolds.
    let snap = snapshot(&wb, cx);
    let (fold, n) = get(&snap, Role::Button, "Fold line 1");
    assert_eq!(n.is_expanded(), Some(true));
    assert!(n.supports_action(Action::Collapse) && !n.supports_action(Action::Expand));
    act(&wb, cx, &snap, fold, Action::Collapse, None);
    let (_, n) = get(&snapshot(&wb, cx), Role::Button, "Unfold line 1");
    assert_eq!(n.is_expanded(), Some(false));
}

#[gpui::test]
fn conflict_links_and_git_marks(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = crate::qa_integration_tests::repo("a11y-editor-marks");
    std::fs::write(dir.join("src/lib.rs"), (1..=10).map(|i| if i == 3 { "// changed\n".to_string() } else { format!("// satır {i}\n") }).collect::<String>())
        .unwrap();
    let file = dir.join("merge.txt");
    std::fs::write(&file, "top\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("src/lib.rs"), cx);
    wait_ticking(cx, "the Git mark", |cx| find(&snapshot(&wb, cx), Role::Label, |l| l == "Line 3 changed").is_some());

    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    let snap = snapshot(&wb, cx);
    let (link, n) = get(&snap, Role::Link, "Accept Current Change");
    assert_eq!(n.description(), Some("Conflict at line 2"));
    hit(&snap, link);
    assert!(find(&snap, Role::Link, |l| l == "Open Merge Editor").is_some());
    act(&wb, cx, &snap, link, Action::Click, None);
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "top\nours\n");
}

#[gpui::test]
fn send_request_and_the_color_picker(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = fresh("a11y-editor-picker");
    std::fs::write(dir.join("api.http"), "GET https://example.com/\n").unwrap();
    std::fs::write(dir.join("a.css"), "a { color: #ff0000; }\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("api.http"), cx);
    let snap = snapshot(&wb, cx);
    let (send, n) = get(&snap, Role::Button, "Send Request");
    assert_eq!(n.description(), Some("line 1"));
    hit(&snap, send);

    let ed = crate::qa_editor_tests::open(&wb, &dir.join("a.css"), cx);
    ed.update(cx, |e, cx| e.set_cursor_for_test(13, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Pick Color…", window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let (opacity, n) = get(&snap, Role::Slider, "Opacity");
    assert_eq!((n.numeric_value(), n.max_numeric_value()), (Some(100.), Some(100.)));
    assert!(n.supports_action(Action::Decrement) && n.supports_action(Action::SetValue));
    for name in ["Hue", "Saturation", "Brightness"] {
        assert!(find(&snap, Role::Slider, |l| l == name).is_some(), "{name}");
    }
    act(&wb, cx, &snap, opacity, Action::SetValue, Some(ActionData::NumericValue(50.)));
    let text = ed.read_with(cx, |e, _| e.text());
    assert!(text.starts_with("a { color: #ff000080"), "{text}");
    let snap = snapshot(&wb, cx);
    assert_eq!(get(&snap, Role::Slider, "Opacity").1.numeric_value(), Some(50.));
    let (rgb, _) = get(&snap, Role::RadioButton, "RGB");
    act(&wb, cx, &snap, rgb, Action::Click, None);
    assert!(ed.read_with(cx, |e, _| e.text()).contains("rgb"), "the notation changed");
}

#[gpui::test]
fn the_terminal_find_bar(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx));
    cx.run_until_parked();
    let term = wb.read_with(cx, |wb, _| wb.terminals[0].clone());
    term.update_in(cx, |t, window, cx| t.find_for_test("needle", window, cx));
    cx.run_until_parked();
    let snap = snapshot(&wb, cx);
    let (field, n) = get(&snap, Role::SearchInput, "Find");
    assert_eq!(n.value(), Some("needle"));
    hit(&snap, field);
    for name in ["Previous Match", "Next Match"] {
        assert!(find(&snap, Role::Button, |l| l == name).is_some(), "{name}");
    }
    let group = snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Group && n.label() == Some("Find in Terminal")).expect("the bar");
    let close = group.1.children().iter().copied().find(|id| find(&snap, Role::Button, |l| l == "Close").is_some_and(|(c, _)| c == *id));
    let close = close.unwrap_or_else(|| *group.1.children().last().unwrap());
    act(&wb, cx, &snap, close, Action::Click, None);
    assert_eq!(term.update_in(cx, |t, window, cx| t.find_state_for_test(window, cx)).1, 0, "the bar closed");
    assert!(find(&snapshot(&wb, cx), Role::SearchInput, |l| l == "Find").is_none());
}

/// The terminal's history: scrolled in pages from the tree, offered only toward lines that exist.
#[test]
fn the_terminal_history_scrolls_from_the_tree() {
    let snap = |scroll: (usize, usize)| {
        let s = Snapshot { terminal: Some(TerminalSnap { label: "Terminal 1 of 1".into(), runs: Vec::new(), cursor: None, scroll }), ..Snapshot::default() };
        s.to_tree().nodes.into_iter().find(|(id, _)| *id == TERMINAL).unwrap().1
    };
    let n = snap((5, 20));
    assert_eq!((n.scroll_y(), n.scroll_y_max()), (Some(15.), Some(20.)));
    assert!(n.supports_action(Action::ScrollUp) && n.supports_action(Action::ScrollDown));
    let n = snap((0, 20));
    assert!(n.supports_action(Action::ScrollUp) && !n.supports_action(Action::ScrollDown), "at the bottom");
    let n = snap((0, 0));
    assert!(!n.supports_action(Action::ScrollUp) && n.scroll_y().is_none(), "no history");
}
