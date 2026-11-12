//! Outline and breadcrumb symbols in a real workbench: line patterns without a server, the language server's
//! `documentSymbol` with a fallback to the patterns, the filter, the sort orders, following the cursor, and the sibling
//! picker the breadcrumbs open.

use accesskit::Role;
use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::json;

use super::*;
use crate::qa_integration_tests::{setup_with, wait_ticking};
use crate::qa_lsp_settings_tests::{dir, open_project};
use crate::test_lsp::FakeServer;

const SHAPES: &str = "struct Shape;\nimpl Shape {\n    fn new() -> Self { Shape }\n    fn area(&self) -> u32 {\n        0\n    }\n}\nfn helper() {}\n";

fn symbols(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<(String, usize)> {
    wb.read_with(cx, |wb, _| wb.outline_state_for_test().1)
}

fn source(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Source {
    wb.read_with(cx, |wb, _| wb.outline_state_for_test().0)
}

fn pairs(list: &[(&str, usize)]) -> Vec<(String, usize)> {
    list.iter().map(|(n, d)| (n.to_string(), *d)).collect()
}

fn at_line(wb: &Entity<Workbench>, cx: &mut VisualTestContext, line: usize) {
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(line, 0, cx));
    cx.run_until_parked();
}

/// A project with `src/shapes.rs` open, on the line patterns only (language servers are off in tests).
fn patterns<'a>(cx: &'a mut TestAppContext, name: &str) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    let dir = crate::qa_workbench_tests::project(name);
    std::fs::write(dir.join("src/shapes.rs"), SHAPES).unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &dir.join("src/shapes.rs"), cx);
    wait_ticking(cx, "the outline", |cx| symbols(&wb, cx).len() == 5);
    (wb, cx)
}

#[gpui::test]
fn the_patterns_give_a_nested_outline_when_no_server_runs(cx: &mut TestAppContext) {
    let (wb, cx) = patterns(cx, "outline-patterns");
    assert_eq!(symbols(&wb, cx), pairs(&[("Shape", 0), ("Shape", 0), ("new", 1), ("area", 1), ("helper", 0)]));
    assert_eq!(source(&wb, cx), Source::Patterns);
}

#[gpui::test]
fn the_filter_the_sort_orders_and_the_follow_switch_shape_the_rows(cx: &mut TestAppContext) {
    let (wb, cx) = patterns(cx, "outline-view");
    let rows = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.outline_rows_for_test(cx));
    wb.update(cx, |wb, cx| wb.ensure_outline_filter(cx));
    // Filter as you type: the field's text decides the rows (and the enclosing impl stays as context).
    let field = wb.read_with(cx, |wb, _| wb.outline.filter.clone()).unwrap();
    field.update(cx, |f, cx| f.set_text("ARE", cx));
    assert_eq!(rows(&wb, cx), pairs(&[("Shape", 0), ("area", 1)]));
    field.update(cx, |f, cx| f.set_text("", cx));
    // Sort by name, then by kind: flat orders.
    wb.update(cx, |wb, cx| wb.change_outline_view(|v| v.sort = Sort::Name, cx));
    assert_eq!(rows(&wb, cx), pairs(&[("area", 0), ("helper", 0), ("new", 0), ("Shape", 0), ("Shape", 0)]));
    wb.update(cx, |wb, cx| wb.change_outline_view(|v| v.sort = Sort::Kind, cx));
    assert_eq!(rows(&wb, cx).into_iter().map(|(n, _)| n).collect::<Vec<_>>(), ["Shape", "Shape", "area", "helper", "new"].map(String::from));
    // The choice is kept for the workspace and read back.
    wb.update(cx, |wb, cx| {
        wb.change_outline_view(|v| v.follow = false, cx);
        wb.outline.view = View::default();
        wb.outline.view_for = None;
        wb.load_outline_view();
    });
    assert_eq!(wb.read_with(cx, |wb, _| (wb.outline.view.sort, wb.outline.view.follow)), (Sort::Kind, false));
}

#[gpui::test]
fn the_list_follows_the_cursor_only_while_follow_is_on(cx: &mut TestAppContext) {
    let (wb, cx) = patterns(cx, "outline-follow");
    let followed = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.outline.followed.get());
    at_line(&wb, cx, 4);
    wait_ticking(cx, "the list to follow", |cx| followed(&wb, cx) == Some(3));
    wb.update(cx, |wb, cx| wb.change_outline_view(|v| v.follow = false, cx));
    at_line(&wb, cx, 7);
    cx.run_until_parked();
    assert_eq!(followed(&wb, cx), None, "with follow off the list does not move");
    wb.update(cx, |wb, cx| wb.change_outline_view(|v| v.follow = true, cx));
    wait_ticking(cx, "the list to follow again", |cx| followed(&wb, cx) == Some(4));
}

#[gpui::test]
fn the_breadcrumbs_show_the_chain_and_a_segment_opens_its_siblings(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (wb, cx) = patterns(cx, "outline-crumbs");
    at_line(&wb, cx, 4);
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let tree = snap.to_tree();
    let crumb = |label: &str| {
        tree.nodes.iter().find(|(_, n)| n.role() == Role::Button && n.label() == Some(label) && n.description().is_some_and(|d| d.contains("Choose another")))
    };
    assert!(crumb("Shape").is_some(), "the impl is the first segment");
    let (_, area) = crumb("area").expect("the method is the last segment");
    assert_eq!(area.description(), Some("function. Choose another symbol at this level."), "line patterns call a method a function");

    // The command opens the siblings of the innermost segment; choosing one goes there.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Go: Pick Symbol at the Cursor's Level", window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)), Some(vec!["new".to_string(), "area".to_string()]));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)).map(|(l, _)| l), Some(3), "`new` is on line 3");
}

#[gpui::test]
fn the_language_server_symbols_replace_the_patterns_and_a_failure_keeps_them(cx: &mut TestAppContext) {
    let d = dir("outline-server");
    let root = d.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), SHAPES).unwrap();
    let range = |a: u32, b: u32| json!({ "start": { "line": a, "character": 0 }, "end": { "line": b, "character": 1 } });
    let name_at = |a: u32| json!({ "start": { "line": a, "character": 7 }, "end": { "line": a, "character": 12 } });
    let answer = json!([
        { "name": "Shape", "kind": 23, "range": range(0, 6), "selectionRange": name_at(0), "children": [
            { "name": "new", "kind": 6, "range": range(2, 2), "selectionRange": name_at(2) },
            { "name": "area", "kind": 6, "range": range(3, 5), "selectionRange": name_at(3) } ] },
        { "name": "helper", "kind": 12, "range": range(7, 7), "selectionRange": name_at(7) },
    ]);
    let Some(fake) = FakeServer::new(
        &d.join("server"),
        &json!({ "capabilities": { "documentSymbolProvider": true }, "responses": { "textDocument/documentSymbol": { "result": answer } } }),
    ) else {
        return;
    };
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: true, lsp_servers: vec![("rust".into(), Some(fake.command().into()))], ..Settings::default() });
    open_project(&wb, cx, &root);
    wait_ticking(cx, "the server's symbols", |cx| source(&wb, cx) == Source::Server);
    assert_eq!(symbols(&wb, cx), pairs(&[("Shape", 0), ("new", 1), ("area", 1), ("helper", 0)]));
    assert!(!fake.received("textDocument/documentSymbol").is_empty());
    // The server's ranges give the chain: the cursor inside `area` is in `Shape`'s `area`.
    at_line(&wb, cx, 4);
    let chain = wb.read_with(cx, |wb, _| {
        crate::outline::model::path_at(&wb.outline.symbols, 4).into_iter().map(|i| wb.outline.symbols[i].name.clone()).collect::<Vec<_>>()
    });
    assert_eq!(chain, ["Shape", "area"]);
}

#[gpui::test]
fn a_server_that_fails_or_has_no_symbols_leaves_the_line_patterns(cx: &mut TestAppContext) {
    let d = dir("outline-server-fails");
    let root = d.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), SHAPES).unwrap();
    // `capabilities` offers symbols but the method answers an error: the patterns stay.
    let Some(fake) = FakeServer::new(&d.join("server"), &json!({ "capabilities": { "documentSymbolProvider": true } })) else { return };
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: true, lsp_servers: vec![("rust".into(), Some(fake.command().into()))], ..Settings::default() });
    open_project(&wb, cx, &root);
    wait_ticking(cx, "the request", |_| !fake.received("textDocument/documentSymbol").is_empty());
    cx.run_until_parked();
    assert_eq!(source(&wb, cx), Source::Patterns);
    assert_eq!(symbols(&wb, cx).len(), 5);
}

#[gpui::test]
fn a_server_without_symbol_support_is_not_asked(cx: &mut TestAppContext) {
    let d = dir("outline-server-none");
    let root = d.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), SHAPES).unwrap();
    let Some(fake) = FakeServer::new(&d.join("server"), &json!({ "capabilities": {} })) else { return };
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: true, lsp_servers: vec![("rust".into(), Some(fake.command().into()))], ..Settings::default() });
    open_project(&wb, cx, &root);
    wait_ticking(cx, "the outline", |cx| symbols(&wb, cx).len() == 5);
    cx.run_until_parked();
    assert!(fake.received("textDocument/documentSymbol").is_empty());
}

/// The picker (here the breadcrumbs' siblings) stays inside a window narrower than its width, left edge included.
#[gpui::test]
fn the_picker_stays_inside_a_narrow_window(cx: &mut TestAppContext) {
    let (wb, cx) = patterns(cx, "outline-narrow-picker");
    for width in [1280., 560., 520.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.)));
        at_line(&wb, cx, 4);
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Go: Pick Symbol at the Cursor's Level", window, cx));
        cx.run_until_parked();
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.)));
        cx.run_until_parked();
        let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx));
        let bounds = cx.debug_bounds("picker-layer").unwrap_or_else(|| panic!("the picker is drawn; labels {labels:?}"));
        assert!(bounds.left() >= gpui::px(0.) && bounds.right() <= gpui::px(width), "{width}: {bounds:?}");
        cx.simulate_keystrokes("escape");
    }
}

/// After an edit the server's last answer stays (marked stale) until its answer for the new text arrives: the symbols
/// never go back to the line patterns in between. The server's answer is applied by the test (the server is not asked
/// for the edit), so nothing depends on how fast a real one answers.
#[gpui::test]
fn an_edit_keeps_the_servers_symbols_on_show_until_the_new_answer(cx: &mut TestAppContext) {
    let d = dir("outline-stale");
    let root = d.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), SHAPES).unwrap();
    let range = |a: u32, b: u32| json!({ "start": { "line": a, "character": 0 }, "end": { "line": b, "character": 1 } });
    let answer = |name: &str| json!([{ "name": name, "kind": 5, "range": range(0, 7), "selectionRange": range(0, 0) }]);
    let Some(fake) = FakeServer::new(
        &d.join("server"),
        &json!({ "capabilities": { "documentSymbolProvider": true }, "responses": { "textDocument/documentSymbol": { "result": answer("OnlyTheServerKnowsThis") } } }),
    ) else {
        return;
    };
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: true, lsp_servers: vec![("rust".into(), Some(fake.command().into()))], ..Settings::default() });
    open_project(&wb, cx, &root);
    wait_ticking(cx, "the server's symbols", |cx| source(&wb, cx) == Source::Server);
    let state = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| {
        wb.read_with(cx, |wb, _| (wb.outline.stale, wb.outline.source, wb.outline.symbols.iter().map(|s| s.name.clone()).collect::<Vec<_>>()))
    };
    // An edit, re-extracted at once (no wait for the typing pause), without asking the server.
    let edit_and_extract = |wb: &Entity<Workbench>, cx: &mut VisualTestContext, text: &str| {
        wb.update(cx, |wb, _| wb.outline.ask_paused = true);
        wb.update_in(cx, |wb, window, cx| wb.focus_active_editor(window, cx));
        cx.simulate_input(text);
        wb.update(cx, |wb, cx| {
            wb.outline.computed_at = None;
            wb.sync_outline(cx);
        });
    };
    edit_and_extract(&wb, cx, "// edit\n");
    assert_eq!(state(&wb, cx), (true, Source::Server, vec!["OnlyTheServerKnowsThis".to_string()]), "the last answer stays, marked stale");
    // The new answer replaces it.
    wb.update(cx, |wb, cx| {
        let key = wb.outline.key.clone();
        wb.apply_server_answer(&key, Ok(answer("Fresh")), cx);
    });
    assert_eq!(state(&wb, cx), (false, Source::Server, vec!["Fresh".to_string()]));
    // Another edit; this time the server fails: the line patterns take over, once.
    edit_and_extract(&wb, cx, "// again\n");
    assert!(state(&wb, cx).0);
    wb.update(cx, |wb, cx| {
        let key = wb.outline.key.clone();
        wb.apply_server_answer(&key, Err("no answer".into()), cx);
    });
    let (stale, source_now, names) = state(&wb, cx);
    assert_eq!((stale, source_now, names.len()), (false, Source::Patterns, 5));
    // An answer for text that has moved on is dropped.
    wb.update(cx, |wb, cx| wb.apply_server_answer(&Some(("elsewhere".into(), 0)), Ok(answer("Late")), cx));
    assert_eq!(source(&wb, cx), Source::Patterns);
}
