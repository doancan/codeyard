//! The Search view and the Problems panel through the accessibility tree only: the query typed and its options set from
//! the tree, the results tree (files expand and collapse, a match opens), an error announced, and the path a screen
//! reader user takes from a search result to the diagnostic in that file and its quick fix.

use accesskit::{Action, ActionData, ActionRequest, Live, Node, NodeId, Role, Toggled, TreeId};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_integration_tests::wait_ticking;

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn find(snap: &Snapshot, role: Role, label: &str) -> Option<(NodeId, Node)> {
    snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == role && n.label() == Some(label))
}

fn node(snap: &Snapshot, role: Role, label: &str) -> (NodeId, Node) {
    find(snap, role, label).unwrap_or_else(|| panic!("{role:?} {label:?} is in the tree"))
}

fn act(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId, action: Action, data: Option<ActionData>) {
    let req = ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&req, snap, window, cx));
    cx.run_until_parked();
}

fn run(wb: &Entity<Workbench>, cx: &mut VisualTestContext, command: &str) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(command, window, cx));
    cx.run_until_parked();
}

/// Types `text` into the Search view's query from the tree and waits for the search to finish.
fn search(wb: &Entity<Workbench>, cx: &mut VisualTestContext, text: &str) -> Snapshot {
    let snap = snapshot(wb, cx);
    let (query, _) = node(&snap, Role::SearchInput, "Search");
    act(wb, cx, &snap, query, Action::SetValue, Some(ActionData::Value(text.into())));
    wait_ticking(cx, "the search", |cx| wb.read_with(cx, |wb, cx| wb.search_summary_for_test(cx)).is_some());
    snapshot(wb, cx)
}

#[gpui::test]
fn the_search_view_is_driven_from_the_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = crate::qa_workbench_tests::project("a11y-search-view");
    std::fs::write(dir.join("src/main.rs"), "fn main() {\n    let needle = 1;\n    let Needle = 2;\n}\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    run(&wb, cx, "Search: Find in Files");
    let snap = snapshot(&wb, cx);
    let (query, _) = node(&snap, Role::SearchInput, "Search");
    assert_eq!(snap.to_tree().focus, query, "the query field has the keyboard, and the tree says so");

    // Typed from the tree: it searches as typing does; the results are a tree of files and matches.
    let snap = search(&wb, cx, "needle");
    let (file, f) = node(&snap, Role::TreeItem, "main.rs");
    assert_eq!((f.level(), f.is_expanded(), f.value()), (Some(1), Some(true), Some("2 matches")));
    assert!(f.supports_action(Action::Collapse));
    let (_, m) = node(&snap, Role::TreeItem, "let needle = 1;");
    assert_eq!((m.level(), m.description()), (Some(2), Some("line 2, column 9")));
    assert!(find(&snap, Role::Status, "2 results in 1 file").is_some_and(|(_, n)| n.live() == Some(Live::Polite)));
    // Each part has the frame it is drawn at; a hit test finds the row, and a file's buttons are inside it.
    let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
    let state = tree.state();
    for (role, label) in [(Role::SearchInput, "Search"), (Role::CheckBox, "Match case"), (Role::TreeItem, "main.rs"), (Role::TreeItem, "let needle = 1;")] {
        let b = node(&snap, role, label).1.bounds().unwrap_or_else(|| panic!("{label} has a frame"));
        let at = accesskit::Point::new(b.x0 + 2., (b.y0 + b.y1) / 2.);
        let hit = state.root().node_at_point(at, &|_| accesskit_consumer::FilterResult::Include).and_then(|n| n.label());
        assert_eq!(hit.as_deref(), Some(label), "hit test at the start of {label}");
    }
    let row = f.bounds().unwrap();
    let dismiss = node(&snap, Role::Button, "Dismiss this file from the results").1.bounds().expect("laid out while hidden");
    assert!(dismiss.x0 >= row.x0 && dismiss.x1 <= row.x1 && dismiss.y0 >= row.y0 && dismiss.y1 <= row.y1, "{dismiss:?} in {row:?}");

    // Match case from the tree: one result left; the option reads as checked.
    let (case, c) = node(&snap, Role::CheckBox, "Match case");
    assert_eq!(c.toggled(), Some(Toggled::False));
    act(&wb, cx, &snap, case, Action::Click, None);
    wait_ticking(cx, "the case-sensitive search", |cx| wb.read_with(cx, |wb, cx| wb.search_summary_for_test(cx)).is_some_and(|s| s.0 == 1));
    let now = snapshot(&wb, cx);
    assert_eq!(node(&now, Role::CheckBox, "Match case").1.toggled(), Some(Toggled::True));
    // The same press from the old tree (which showed it off) does not turn it off again.
    act(&wb, cx, &snap, case, Action::Click, None);
    assert_eq!(node(&snapshot(&wb, cx), Role::CheckBox, "Match case").1.toggled(), Some(Toggled::True));

    // Collapse and expand the file from the tree.
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, file, Action::Collapse, None);
    let snap = snapshot(&wb, cx);
    assert_eq!(node(&snap, Role::TreeItem, "main.rs").1.is_expanded(), Some(false));
    assert!(find(&snap, Role::TreeItem, "let needle = 1;").is_none(), "its matches are hidden");
    act(&wb, cx, &snap, file, Action::Expand, None);
    let snap = snapshot(&wb, cx);

    // A match opens where it is.
    let (row, _) = node(&snap, Role::TreeItem, "let needle = 1;");
    act(&wb, cx, &snap, row, Action::Click, None);
    wait_ticking(cx, "the match to open", |cx| wb.read_with(cx, |wb, _| wb.active_editor().is_some()));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_editor().unwrap().read(cx).cursor_position().0), 2);
    assert_eq!(node(&snapshot(&wb, cx), Role::TreeItem, "let needle = 1;").1.is_selected(), Some(true), "the current match");

    // A pattern error interrupts: an assertive alert, no results.
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, node(&snap, Role::CheckBox, "Regular expression").0, Action::Click, None);
    let snap = snapshot(&wb, cx);
    act(&wb, cx, &snap, node(&snap, Role::SearchInput, "Search").0, Action::SetValue, Some(ActionData::Value("needle(".into())));
    let alert = |cx: &mut VisualTestContext| snapshot(&wb, cx).to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Alert);
    wait_ticking(cx, "the error", |cx| alert(cx).is_some());
    assert_eq!(alert(cx).unwrap().1.live(), Some(Live::Assertive));
}

/// The path a screen reader user takes: search for the code, open the result, find its diagnostic in Problems, ask for
/// its fixes and apply one, all through the tree; the fixed file has no problem left, which is announced.
#[gpui::test]
fn search_result_to_diagnostic_to_fix(cx: &mut TestAppContext) {
    // The server is a Python script started by its interpreter (`python` on Windows runners, `python3` elsewhere).
    let Some(python) = ["python3", "python"].into_iter().find(|p| std::process::Command::new(p).arg("-V").output().is_ok_and(|o| o.status.success())) else {
        crate::test_sshd::skip_missing("python3");
        return;
    };
    let _en = crate::i18n::LangGuard::set("en");
    let dir = crate::qa_workbench_tests::fresh("a11y-search-fix");
    let script = dir.join("fix-ls.py");
    std::fs::write(&script, FIX_SERVER).unwrap();
    std::fs::write(dir.join("a.c"), "int main(void) { return x; }\n").unwrap();
    let settings = Settings {
        lsp_enabled: true,
        lsp_servers: vec![("c".into(), Some((python.to_string(), vec![script.to_string_lossy().into_owned()]).into()))],
        ..Settings::default()
    };
    let (wb, cx) = crate::qa_integration_tests::setup_with(cx, settings);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    run(&wb, cx, "Search: Find in Files");

    // Search → result.
    let snap = search(&wb, cx, "return x");
    let (row, _) = node(&snap, Role::TreeItem, "int main(void) { return x; }");
    act(&wb, cx, &snap, row, Action::Click, None);
    wait_ticking(cx, "the file to open", |cx| wb.read_with(cx, |wb, _| wb.active_editor().is_some()));

    // Result → diagnostic.
    run(&wb, cx, "View: Problems");
    wait_ticking(cx, "the diagnostic", |cx| find(&snapshot(&wb, cx), Role::ListItem, "Error: x undeclared").is_some());
    let snap = snapshot(&wb, cx);
    assert!(find(&snap, Role::Status, "1 errors, 0 warnings").is_some_and(|(_, n)| n.live() == Some(Live::Polite)), "the count is announced");
    let (problem, p) = node(&snap, Role::ListItem, "Error: x undeclared");
    let fix = *p.children().iter().find(|id| snap.to_tree().nodes.iter().any(|(i, n)| i == *id && n.label() == Some("Quick Fix…"))).expect("its fix button");
    let _ = problem;
    assert!(p.bounds().is_some_and(|b| b.width() > 0.), "the problem's row has the frame it is painted at");

    // Diagnostic → fix: the fixes are a picker; choosing one applies it.
    act(&wb, cx, &snap, fix, Action::Click, None);
    wait_ticking(cx, "the fixes", |cx| snapshot(&wb, cx).picker.is_some_and(|p| !p.list.rows.is_empty()));
    let snap = snapshot(&wb, cx);
    let (option, _) = node(&snap, Role::ListBoxOption, "Declare x");
    act(&wb, cx, &snap, option, Action::Click, None);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_ticking(cx, "the fix applied", |cx| ed.read_with(cx, |e, _| e.text().starts_with("int x;\n")));
    wait_ticking(cx, "no problem left", |cx| find(&snapshot(&wb, cx), Role::Status, "No problems.").is_some());
    let snap = snapshot(&wb, cx);
    assert!(snap.picker.is_none(), "the picker closed");
    assert_eq!(snap.focus, FocusSnap::Editor, "the keyboard is back in the editor");
}

/// A language server for C that reports `x` undeclared until a line declares it, and offers the fix that does.
const FIX_SERVER: &str = r#"#!/usr/bin/env python3
import sys, json
def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            break
        k, v = line.split(b':', 1)
        headers[k.strip().lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers[b'content-length'])))
def send(m):
    b = json.dumps(m).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b)
    sys.stdout.buffer.flush()
def publish(uri, text):
    diags = []
    if 'int x;' not in text:
        col = text.find('x;')
        diags.append({'range': {'start': {'line': 0, 'character': col}, 'end': {'line': 0, 'character': col + 1}}, 'severity': 1, 'message': 'x undeclared'})
    send({'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics', 'params': {'uri': uri, 'diagnostics': diags}})
while True:
    m = read()
    method = m.get('method')
    if method == 'initialize':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': {'capabilities': {'textDocumentSync': 1, 'codeActionProvider': True}}})
    elif method == 'textDocument/didOpen':
        publish(m['params']['textDocument']['uri'], m['params']['textDocument']['text'])
    elif method == 'textDocument/didChange':
        publish(m['params']['textDocument']['uri'], m['params']['contentChanges'][-1]['text'])
    elif method == 'textDocument/codeAction':
        uri = m['params']['textDocument']['uri']
        edit = {'changes': {uri: [{'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 0}}, 'newText': 'int x;\n'}]}}
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [{'title': 'Declare x', 'kind': 'quickfix', 'edit': edit}]})
    elif method == 'shutdown':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': None})
"#;
